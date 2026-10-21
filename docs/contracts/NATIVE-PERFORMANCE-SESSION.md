# Native recorded-performance worker

October 20, 2026. Implementation contract extending [Finish](NATIVE-PERFORMANCE-FINISH.md). This is not runtime acceptance. Preserve existing v1/v2 originals and installed workflows. Output bus only: never open native input, including virtual input.

## Ownership and concurrency

A `PerformanceSession` owns one starting intent, one sole `LiveAdmission`, one concrete native-I/O owner thread, and one off-callback writer/drainer. The concrete I/O owner constructs and retains `cpal::Stream` on that thread, so platform `!Send` stream restrictions are respected. It creates the actual renderer and error guard and transfers them into the actual native closures. It hands sole admission back to the serialized worker only after the descriptor/journal is created and the drainer is running. Start/Status/Finish/Stop handlers never join, wait on SQL, pause/drop a stream or block on a background reply.

Use bounded channels and shared status snapshots, with no mutex/file/IPC operation on a native callback. The writer publishes complete/durable counters outside SQL calls. Fault/storage/tap error immediately cancels admission and taps, retains exact first errors/uncertain requests and does not resume or fall back to unsaved audition. A failed writer cannot mint normal-seal authority.

Keep admission, ring and lookup-table backing alive until actual renderer, actual error closure and tap producer destruction. Every admission transfer is wrapped in a nonclone lease: if dropped before all real proofs, cancel and deliberately retain/leak that bounded owner while retiring the worker generation. Cover failed ready-channel send and parent EOF during startup or stalled native teardown. Do not depend on the tap's own retention guard to protect the separate ingress/tables.

The worker's parent heartbeat watchdog alone is insufficient while heartbeats continue. Separate starting/finishing/native-stop/draining/sealing deadlines set the worker retirement latch regardless of parent traffic. A stalled filesystem/OS call cannot be interrupted safely by a Rust thread. Retirement preserves stage identity and durable prefix; it is not successful Stop or Finish. Never admit another session while unproven owners remain.

## Stable identity and readiness

`PerformanceStart { id: LiveId, output_device: Option<String>, gui_clock: LiveClock }` is an immutable full intent. The same ID with any changed field rejects; exact active retry returns current status without opening a second stream. Map ID deterministically to a private capture directory and take identity, using exclusive stage creation. Any existing stage encountered by a new worker is inspection/recovery-only, even if its Start fields appear similar. No recording resumes into it.

A prompt Starting response retains the attempted capture/take identity before device or SQL preparation finishes. `descriptor_sha256: None` does not assert that a file is absent; creation may be uncertain. Only a completed real journal creation publishes a descriptor hash. No notes or lease credits are admitted until actual Ready. Failure/cancellation retains the identity even when creation/setup is uncertain. Validate canonical ordinary directories and reject redirected stages.

Keep strict performance profile v2 unchanged. The actual device ID/name and native sample conversion format are setup/status/qualification facts, not silently added owned descriptor fields. Original logical F32 format, prepared source identities and exact GUI/render clocks stay in the take. This packet makes no durable cross-worker Start-request replay, archived physical-device-settings or calibrated-alignment claim.

## Finish, Stop and sealing

Healthy parent Finish first closes local note/Panic admission and drains the finite earlier parent-enqueued batches unchanged through worker acceptance. Urgent Stop/EOF can preempt that drain. The worker then calls `begin_finish` on its sole admission, capturing the actual accepted fence under the stable Finish ID; the caller cannot supply its accepted watermark. The existing callback owns the exact hard cut, full PCM, terminal Finish and matching End.

The actual I/O owner observes the immutable cut directly through its real `LiveControl` and tap control, rather than trusting a caller-authored cut or public status booleans. After that ACK it pauses/drops the actual stream, observes actual renderer/error/tap-producer destruction, and rechecks health/cancellation. Only this concrete owner may construct a private, nonclone native-retirement proof bound to Start/take/descriptor/scope/cut. No public constructor or caller bool is authority. Offline fake adapters/tests remain explicitly software evidence.

The writer alone receives that proof, drains through producer destruction and an empty tap, and verifies its final v2 Finish/End/checkpoint, all six prefixes and no partial/error/gap state. It then atomically authorizes normal sealing against cancellation. Cancellation that wins first retains an unsealed original. Once sealing is authorized/in flight, a racing Stop cannot promise no manifest committed: preserve the exact intent and report committed/unknown status. No callback replay or second rendering is allowed to clarify the result.

The normal operation ID is a bounded domain-separated digest of the Finish request/ID, descriptor, complete cut/fence, checkpoint and full index. Retry authenticates the same original rows and mode and returns the same manifest. Recovered/unknown-tail manifests remain distinct. UI attachment is a separate durable operation; native completion does not silently mutate the project.

## Checked normal recording primitive

The recording crate adds strict `CompletePrefix { descriptor_sha256, original_rows_sha256, prefix: RecoveryPrefix }`, `Journal::complete_prefix()` and `Journal::seal_complete_prefix(operation_id, &CompletePrefix)`. This is a trusted lower-layer storage snapshot/API, **not native lifecycle authority**. The engine entry remains private-proof gated.

The primitive requires supported v2 ownership, no v1/null/dual/future marker and no gaps/tails. It verifies exact descriptor and original-row digests, full physical chunk count, complete scan/index and all stream prefixes inside one Immediate finalization transaction. `RecoveryPrefix` supplies only an index/count shape here; it cannot select a shorter prefix. Normal state must be Recording or the exact already-normal committed replay; Interrupted/recovered mode cannot become normal. Changed request/operation/original rows/descriptor/index rejects. Before/after-commit hooks distinguish rollback from committed-but-unacknowledged results. No journal schema migration or automatic recovery is introduced.

## Worker interfaces and acceptance

Version the actual wire/build profile when exposing recorded commands. Public status distinguishes Starting, Recording, Finishing, Stopping, Sealing, Sealed and Interrupted/Failed; includes stable identity, real readiness, separate live/callback/writer progress, native retirement observations, exact Finish intent/cut, manifest or bounded error. Public observations are not seal authority. Publish monotonically and preserve identity through terminal delivery, pending responses and worker retirement.

Required source/software tests: exact/mutated Start retries and existing-stage refusal; no early note admission; one draining owner; real queued renderer data; full accepted fence; Stop during Start/Finish; independently held closures and ready handoff failure; no new session under unproven lifetime; faults/SQL failure/stall/cancellation/EOF; phase deadline despite heartbeats; full-prefix normal seal/replay before/after commit; no recovered-to-normal promotion; exact original bytes/history/archive closure. Independent persistence/realtime review is required.

Then actually qualify native output and paired-process commands using the same built worker, real closure retirement, normal manifest, explicit interruption, retained discovery/recovery and stable retries. No input is needed or authorized. Native controls/UI, listening, latency, thirty-minute recording, metronome/overdub/compensation, MIDI2/MPE, model/plugin work, installers, Windows and final song remain separate requirements until performed.
