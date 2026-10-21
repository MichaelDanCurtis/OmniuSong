# Native saved-performance worker — independent architecture review

October 20, 2026. **Implementation recommendation, not native acceptance.** The [goal](../GOAL.md) still requires the complete Rust workstation and a beautiful, easy, crisp, functional interface. The qualified [Finish prerequisite](RUST-PERFORMANCE-FINISH-SOURCE-REVIEW.md) provides callback evidence and interrupted recovery; the next packet must integrate the actual output stream and persistent writer. This review opens no devices and authorizes no native input, including software-loopback input.

## Recommended ownership

Adopt the coordinator's three-owner split within the existing private audio child process:

| Owner | Exclusive responsibilities | Work it must not perform |
| --- | --- | --- |
| IPC/session owner | Start/Finish identity admission, sole `LiveAdmission`, bounded commands, monotonic status, cancellation, parent liveness and retirement decisions | Device/SQLite waits, joining stalled owner threads, constructing native teardown proof |
| Native I/O thread | Create/select the output device, build/play/own/pause/drop the actual `cpal::Stream`; capture the real renderer and error guard in its callbacks; observe real destruction and produce private proof | Moving `Stream` across threads, opening an input stream, writing journal chunks, claiming a blocked OS call was cancelled |
| Writer/drainer thread | Sole `PerformanceTapOwner` and `PerformanceWriter`, continuous bounded drain, incremental checkpoints, exact pending-write retry, authenticated final transaction | Callback work, stream control, inferring quiescence from an empty queue or supplied counters |

Create the CPAL host/device/configuration and stream on the I/O thread. This avoids relying on `Stream: Send`. Move only the Send admission/owner/control handles through bounded channels. The renderer remains the existing production procedural live renderer with its logical post-mix F32 tap; F32/I16/U16 device conversion retains the current meaning. Selected device/configuration is reported, never changed system-wide. No playback, overdub or monitoring is silently combined with this solo performance session.

The writer stage must exist and the drainer must be ready before `stream.play`. Do not allow notes until successful play, stage/descriptor readiness and admission ownership have all reached the session owner. Silence rendered before the first note is real recorded bus time. A setup/write failure keeps the stage identity and disables recording; there is no fallback to unrecorded audition.

## Minimal session and status interfaces

The exact public DTO spelling belongs to the coordinator; the following boundaries are sufficient:

```rust
struct PerformanceStart {
    id: LiveId,
    output_device: Option<String>,
    gui_clock: LiveClock,
}

// Session owner methods; bounded and nonblocking with respect to OS/SQLite.
PerformanceSession::start_async(root, worker_generation, request)
    -> Result<PerformanceSession, StartRejection>;
PerformanceSession::poll(&mut self) -> PerformanceStatus;
PerformanceSession::admit(&mut self, batch: LiveBatch) -> Result<LiveProgress, LiveSubmitError>;
PerformanceSession::begin_panic(&mut self, scope: LiveScope) -> Result<LiveScope, LiveSubmitError>;
PerformanceSession::begin_finish(&mut self, finish_id: LiveId) -> Result<FinishPending, FinishRejection>;
PerformanceSession::cancel(&self, reason: CancelReason);
```

Start immediately returns a **planned identity** derived from the strict UUID and canonical configured root, such as `captures/<start_id>/take.sqlite3`, with no descriptor hash yet. Missing descriptor identity means creation is unconfirmed, not that the path does not exist: the I/O/writer thread may already have created it. It must not label the planned path as a saved original. Once stage creation succeeds, report the actual descriptor hash. `Ready` additionally contains the real `LiveReady` and selected output/configuration. A returned Starting acknowledgment is neither readiness nor proof of filesystem creation.

Status must keep worker generation, start ID, take/descriptor identity, monotonic status revision and any Finish ID/fence together. It separates these facts: requested/starting, stage ready, recording ready, Finish requested, callback cut, native stopping, native quiesced, writer draining, seal authorized, sealed, interrupted and outcome unknown. Expose received/applied/complete/durable prefixes separately; never use one progress counter for all four. The current `LiveProgress` remains the musical/renderer component, not the whole recording lifecycle.

Typed status validation must bind Finish/cut scope to the same ready/live worker and session, check phase/fault coherence and keep sealed audio interpretation equal to the prepared source. Across status revisions, the parent also checks unchanged Start identity/path, one-way descriptor publication, immutable Finish identity and terminal results. Individually valid fields do not authenticate these cross-field or cross-update relationships.

Use one terminal-preserving status mailbox per session plus bounded unique handoff channels. Coalescing progress must not overwrite the first failure, original identity, exact Finish intent or committed manifest. A full event queue cannot strand the only admission or proof behind optional meter updates. The IPC handler polls completed messages and sends bounded snapshots; it does not wait for a thread join. All terminal results remain available until explicitly acknowledged/replaced under the session contract.

## Start identity, retries and original format

The coordinator freezes these rules:

- Within one worker, a Start ID binds the **entire exact original Start request**, including output selection and GUI domain/epoch/ratio. Same-ID/same-request returns the existing state; changed fields reject. No second native renderer is created for a retry.
- Stage creation uses an exclusive fresh directory/file barrier. Any existing stage in a new worker is **inspect-only**, never native resume or recreation. There is no cross-worker durable Start-request replay claim. Empty/partially created directories are retained evidence of an interrupted attempt, not silently reused.
- Keep the strict v2 performance descriptor/configuration exact. Its six original streams already contain logical F32 interpretation, full GUI clocks and local source identities. Actual device ID/format belongs to live status and qualification receipts in this packet; it is **not archived physical device provenance**. A later durable sidecar would need a versioned format and explicit archive closure. Do not insert new owned v2 fields silently.
- Bump and negotiate the private worker profile/build before adding recorded commands or exposing `Finishing`/`FinishedCut` through IPC. An old peer must fail visibly, not treat the new states as ordinary audition.

Initial failures with no created stage can be known setup failures; failures after creation must retain the identity. A worker crash does not erase which path was planned, but the next process must inspect actual existence/integrity. Parent/UI retry must not interpret lost Start acknowledgment as permission to mint another capture automatically.

## Ownership across startup, handoff and interruption

The existing `LiveAdmission` retains the ingress producer/tables and tap backing off callback. Moving it to the IPC owner is sound only if it stays alive until real teardown. The tap's early-drop leak guard alone does not retain all ingress/table backing.

Use a non-cloneable admission owner wrapper around `Option<LiveAdmission>`. Its destructor checks the actual lifetime flags. If teardown is unproven, it sets cancellation and retains/leaks the **whole admission** until process retirement instead of allowing a callback to become the last backing owner. Apply this rule to failed Ready sends, messages left in a disconnected channel, startup cancellation and Runtime drop on EOF—not just the normal Stop method. Once renderer and error closures and tap producer are proven gone, ordinary off-callback destruction is safe.

**Concrete source finding:** the current `LiveRenderer::drop` drops the recording wrapper and ingress consumer, then sets `render_done`; its remaining `Arc<Tables>` and `Arc<LiveControl>` fields auto-drop after that destructor body. An asynchronous callback-closure destructor can therefore publish the inner flag before all backing references are gone. The new native owner must use a complete closure acknowledgment: for example, a concrete callback wrapper explicitly takes and fully drops `Option<LiveRenderer>` before publishing its outer acknowledgment. The admission lease and native proof use that outer flag, retaining a control owner through acknowledgment/proof. Merely checking the current inner flag is insufficient for the new ownership handoff. This is a source-level requirement, not an observed device failure.

The I/O thread needs a crate-private, fault-vetoing observer of the actual shared Finish publication after the admission handoff. Add a read-only observer on `LiveControl` or an equivalent immutable handle. It must validate the actual published cut/fence; a `TapFinish` supplied by the IPC owner is only an expectation to compare, never authority. This does not require a public proof constructor or caller success flag.

On write validation/error or ambiguous commit, the drainer immediately sets shared cancel and live/tap fault, permanently closes admission and preserves the pending request/path. Retrying an exact write may authenticate a durable prefix, but must never resume capture. I/O teardown can proceed while SQLite remains blocked. A blocked drainer retains its sole consumer and journal until it returns or the process retires.

## Healthy Finish and cancellation

Healthy parent Finish first closes the GUI admission gate, resolves its in-flight exchange and forwards the finite queue of already parent-enqueued batches unchanged. It waits for their worker admission before requesting the worker's immutable fence. Preserve original lease tokens and event clocks. Reject/uncertain admission cannot be hidden as healthy capture. Urgent Stop, fault or EOF may preempt and explicitly report unforwarded originals as uncaptured.

The worker uses the existing sole `LiveAdmission::begin_finish`; it cannot accept a caller-authored accepted watermark. Pending Panic rejects Finish. The callback drains at most four batches per invocation, preserves complete PCM, emits the first-class hard cut and End, then acknowledges. Callback cut, native retirement and durable seal remain distinct. No user release is synthesized.

After the actual ACK, the I/O owner pauses and drops its own real stream and waits for the **actual** data-renderer, error-closure and tap-producer destruction flags. A play/pause failure preserves ownership and retires the process; it must not retry unsafe teardown or forge proof. A fault/cancel after ACK and before authorization still vetoes normal sealing. Stop/EOF before authorization yields retained unsealed originals, including any valid final Finish record, with unknown tail on later recovery.

### Private retirement proof

`RetirementProof` is crate-private, non-cloneable and has private fields/constructor located beside the concrete native I/O owner. It binds worker/start/take/descriptor identity, the exact immutable Finish and the actual control handles. Its constructor requires:

1. This owner's actual output stream successfully played and belongs to the bound session.
2. Its observed Finish ACK matches the requested ID/scope/fence.
3. Successful pause/drop and acquired real renderer/error/tap-producer destruction acknowledgments.
4. No sticky/pending live or tap fault and no cancellation before proof publication.

Holding/dropping a synthetic `LiveErrorGuard`, returning `true` from an adapter, supplying a `TapSnapshot`, or receiving an empty queue is insufficient. A test-only fake native adapter may exercise state transitions and withheld destruction, but it cannot qualify native proof. The actual owner can pass the proof to the writer through one bounded reliable handoff; IPC does not construct it.

The writer drains until the actual producer is gone **and** the tap is empty, then verifies its terminal checkpoint/complete original rows match the proof's exact cut and fence. It rechecks shared fault/cancel state after those observations. A completed callback with a lost/error writer tail does not qualify.

### Cancellation versus seal authorization

Use one atomic gate shared by session/I/O/writer: an open authorization can transition exactly once to either Cancelled or SealAuthorized. The writer attempts the authorization CAS only after real proof, complete drain and semantic/identity validation. Stop/EOF that wins first leaves unsealed material. The CAS is the explicit linearization point.

After authorization, a racing Stop cannot promise that no normal manifest committed. Output is already quiesced. Report sealing/committed/unknown with the exact Finish identity; keep the writer's immutable pending normal-seal request. Same-process retry authenticates that same authorized attempt and cannot start a second session or change the ending. After worker loss, an already committed normal manifest can be verified/replayed; an unsealed stage cannot reconstruct vanished native lifetime authority and requires explicit interrupted recovery.

## Recording primitive and exact normal replay

The coordinator's proposed no-migration primitive is appropriate:

```rust
struct CompletePrefix {
    descriptor_sha256: String,
    original_rows_sha256: String,
    prefix: RecoveryPrefix,
}
Journal::complete_prefix(&self) -> Result<CompletePrefix>;
Journal::seal_complete_prefix(&mut self, operation_id: &str, expected: &CompletePrefix)
    -> Result<SealedManifest>;
```

`CompletePrefix` is a storage snapshot, **not** native authority. The public lower-layer primitive is a trusted recording-library operation; the engine entry point must remain private and require the real proof/authorization above. `RecoveryPrefix` is reused only as a count/index shape: this API cannot select a shorter prefix or ignore retained tails.

In one Immediate transaction, require supported v2 ownership (reject v1/null/dual/unsupported markers), exact descriptor and original-row digests, healthy complete scan, exact index and **full physical chunks count**, coherent stream-prefix rows and no gaps. For a first normal seal the state is Recording. For replay, the authenticated manifest must already be normal (no interrupted receipt) with the same operation, descriptor, full index/count and originals. Recovered manifests cannot become normal, and a malformed extra physical row cannot disappear merely because a valid prefix exists. Generic seal/recovery guards remain in place.

Derive the operation identity from the exact Finish request, descriptor, callback cut/fence, final checkpoint and complete index. Keep the final immutable request across commit uncertainty. Before/after-commit hooks must preserve the distinction between known rollback and committed-but-unacknowledged; never convert every seal error into “unsealed.” This design needs no new journal table or version if all comparisons and the derived identity remain exact.

## Deadlines, resources and acceptance

Parent contact and session progress are separate liveness signals. The current watchdog is reset by parent traffic; continuing heartbeats would otherwise keep a stuck I/O/SQLite session alive indefinitely. Add absolute, non-renewing phase deadlines for startup, callback Finish, native stopping/closure acknowledgment, draining and sealing. A separate watchdog/retirement decision must continue while an owner blocks. Ordinary recording has no short duration cap; any fault moves it into bounded termination. A wall deadline is not proof that an OS operation ended.

The six maximum-callback frame allowance remains the callback Finish bound. At 8 kHz, 16,384-frame buffers make four queued-drain callbacks exceed eight seconds; choose the worker's wall deadline accordingly, with explicit finite slack. Do not use a universal two-second whole-Finish timeout or continually reset it on progress. Callback packet/ring/voice/format bounds and writer group limits remain the already qualified fixed values. Parent queues, handoffs, progress strings and terminal receipts need explicit bounds; no unbounded per-session thread spawning or repeated Start while an unresolved generation owns devices.

Independent acceptance must cover real production session wiring with device-free fault adapters **and separately the actual native output path**:

- No Ready/note admission before journal/drainer/actual play readiness; identical/conflicting Start retry; existing-stage rejection after worker restart; failed Ready handoff retains backing and identity.
- Finish drains earlier parent batches; Stop during Start/queued Finish; pending Panic; complete cut followed by held data/error closures; late error; exact fence/proof identity mismatch. Missing closure acknowledgment never normal-seals.
- Writer blocked while IPC remains responsive; ring overflow faults before new unrecorded events; callback and drainer cancellation; heartbeat cannot postpone phase retirement; no blocking joins on Drop/EOF.
- Final proof with empty/nonempty tap, mismatched checkpoint, extra valid/bad physical tails, original-row mutation, recovered/v1 manifest rejection, cancellation on either side of authorization, and before/after-commit lost replies with exact retry.
- First fault/identity/terminal receipt survives full parent outbox and process EOF; new admission remains closed until an explicit new session after qualified teardown. Actual helper subprocess tests qualify IPC/process behavior, not CPAL teardown.
- Actual installed same-binary output-only recording, normal Finish/Stop interruption, recovery, attachment, Undo/Redo, restart and complete archive restoration with unchanged originals. Native checks belong to the coordinator after independent source gates; this review performs none.

This is a defensible next implementation boundary because it puts real stream and durable writer ownership in the running worker while leaving its control loop responsive. The risks above require concrete code/tests; another offline boolean or event-list fixture is not an acceptable substitute for the native lifecycle.
