# Independent performance retirement and inspection compatibility review

October 20, 2026. Read-only review of the new retirement-cause latch in [`performance_session.rs`](../../crates/omniusong-engine/src/performance_session.rs), its worker getters, and the bounded-reader compatibility wrappers. Only this review document was written. No production/test source, Cargo, formatting, native device/UI, package or catalog operation was performed.

The latch storage/publication design is sound within its stated scope. The initial review found a response-delivery race, preserved below. **The subsequent worker response fence and parent error-prefix correction resolve that finding in the reviewed source**, as recorded in the follow-up. Native qualification of the diagnostic correction remains pending.

## Bounded readers through compatibility APIs

The original String-returning semantic wrappers selected the internal scanner with `budget: None`. Passing an already bounded reader could therefore omit semantic record/group accounting, or stringify a typed read refusal and catch it as an ordinary damaged-tail `problem`, returning `Ok` with an earlier selection.

The coordinator's narrow correction is appropriate: at the beginning of **both** compatibility wrappers, detect `reader.inspection_budget().is_some()` and delegate to the corresponding typed bounded entrypoint, converting only its final outer error to String. For the sealed reader this check precedes `inspect_snapshot`, avoiding an extra authentication pass. The bounded entrypoints call the internal scanner directly, so delegation does not recurse or allocate another governor. Existing unbounded callers retain their original path and identities.

The added `compatibility_wrappers_cannot_drop_a_bounded_readers_semantic_budget_or_return_partial_success` source regression covers both reader forms, cancellation after open, and a one-group limit on a two-group source. It requires outer errors on the first and repeated call, the exact sticky governor refusal, one completed group before limit refusal, and unchanged source hashes. This is stronger than checking a returned diagnostic string: no usable partial success is returned. Executable results remain the coordinator's evidence.

## Retirement-cause publication

`Shared::retire_with_reason` initializes an immutable `OnceLock<String>` before the Release store setting `retire`. Runtime command and live-action admission first acquire that flag, then use the read-only `PerformanceSession::retirement_error` getter. Thus an observer seeing this performance retirement need not wait for another status snapshot or depend on a free slot in the bounded cancel-reason mailbox.

The first published retirement trigger remains the headline. A distinct existing status error is appended as **prior context**, without overwriting that immutable status error. Prior context is best-effort: the implementation uses `try_lock`, so a held status lock cannot delay latching and may cause that context to be omitted. Later cleanup failures cannot replace the first latched diagnostic. “First” here means the initialization winner, not proof of the earliest physical failure across concurrent producers.

The strings are bounded: the trigger alone is at most 3,900 non-NUL characters; combined output uses at most 2,900 trigger plus 900 prior-context characters and a fixed separator. The worst UTF-8 case stays below 16 KiB. All new latch calls are off callback: deadline handling, admission/native-owner cleanup, pause/destructor failures and writer/owner panic handling. The renderer and error callback do not format strings, allocate this latch, acquire the status mutex, or perform new I/O. Initialization can serialize concurrent off-callback retirement callers; this is not a wait-free callback primitive.

The existing cancellation/seal CAS, callback destruction proof, journal ownership and normal-seal authorization remain separate and unchanged. A retirement reason is neither proof of successful Stop nor authority to seal. The expiry path continues to preserve an already validated `Sealed` observation rather than replacing it with a contradictory uncertain manifest state.

The source tests cover trigger plus distinct prior context, immutable later reads, a held status mutex with bounded Unicode/NUL handling, a full cancel mailbox, and Runtime command/live-action rejection before any new device or project access. The Runtime fixture deliberately obtains the cause without updating the session's cached initial status. Those cases establish the intended getter boundary when a request starts **after** retirement is visible.

## Actionable response-delivery finding

In the initially reviewed `run_worker` loop, an ordinary command can finish with `Ok` just before the independent deadline thread latches retirement. The loop then prepares its update and sends the previously computed normal result. Its subsequent fresh `retire` check immediately calls `exit(75)`. No further command reaches the new getter, so that known cause may still be replaced in the parent by an unexplained EOF/retirement error.

The new Runtime test computes an earlier ordinary completion, injects an expired session, and issues new requests. It correctly verifies their rejection, but does not cover retirement between preparing the final normal response and the loop's exit decision.

Recommended bounded follow-up: prepare the response after update sampling, replace the reply with the latched cause if retirement is already observed, and bind the retirement exit decision to whether **that response** carried the terminal reason. If retirement races after a normal response was prepared, do not treat that already normal response as a delivered terminal cause; a subsequent bounded heartbeat/control rejection must carry it before the terminal exit. Add deterministic tests for retirement before preparation and after a normal preparation. No single atomic flag check can make the following serialization/send indivisible with a concurrent retirement.

This finding concerns delivery of the known cause; it does not show unsafe callback ownership release, original deletion, fabricated Finish, or an incorrect seal. It was reported to the coordinator before this document was frozen. Do not claim unconditional terminal-cause preservation from the latch/getter tests alone.

## Initially reviewed identities

| Source | SHA-256 |
| --- | --- |
| `crates/omniusong-engine/src/performance_session.rs` | `e1222214dc500a4972eb68512c51072700b6b6694e6acedf4e2a715e4b860935` |
| `crates/omniusong-engine/src/worker.rs` | `1566fe532f2bd7ca7abc971e2eb894b9d4b73f5a044319b4741eba6302c7fdc7` |

These identities describe the initial latch review and its delivery finding. Any response-boundary correction requires its own reviewed identity and coordinator gate. This document does not attribute the failed native run to a specific Panic or retirement interleaving, and does not close native, Windows, input, latency, long-run or milestone acceptance.

## Follow-up: response fence and parent disposition

The formatted correction was independently reviewed after the architect froze both sides. No remaining source blocker was found within this response-delivery scope.

`Runtime::response_candidate` samples the ordinary result and update. `finish_response` then acquires the retirement flag at the final preparation fence. If retirement is visible, it replaces the result with the immutable performance cause, retains the correlated request ID, suppresses the update, and records `exit_after_send = true`. A raced Ready therefore cannot publish a new scope or readiness through that terminal response. Update-counter exhaustion also becomes a terminal error rather than retaining an earlier successful command result.

The worker's exit decision now uses that prepared response's flag. If retirement occurs **after** a normal response was prepared, that response remains normal and does not trigger the terminal exit. The next bounded heartbeat/control request rejects admission with the retained cause and prepares the terminal response. This is a preparation boundary, not an atomic send with respect to retirement: it does not retroactively withdraw a response already prepared before the flag became visible.

For a terminal response, the worker attempts its existing three-second bounded send before `exit(75)`. This establishes the cause-bearing send attempt before exit; it cannot guarantee delivery over a disconnected, timed-out or otherwise failed channel. The independent parent-contact watchdog also remains a separate conservative exit path. No successful Stop, Finish, native destruction or seal authority is inferred from the error response.

On the parent side, `exchange` first receives a validated wire payload and checks the private request ID. `validate_response_prefix` preserves the existing command/result-kind predicate, then returns the exact error from `Err + update: None` **before** readiness/performance publication, internal-live update requirements, urgent Stop checks, or Panic acknowledgment processing. Thus a known terminal cause is not replaced by “Missing live authoritative update.” It publishes no new state or acknowledgment. The supervisor's existing terminal `Event::Failed` delivery retains the cause behind any already queued completion and maps it to the active client request, or request ID zero when none is pending. Ordinary `Err + Some(authoritative update)` retains its previous processing path; invalid successful result kinds remain rejected.

The five added boundary regressions exercise retirement between candidate construction and the final fence; retirement after a normal response has been queued; update-counter exhaustion; update-free errors in normal, heartbeat, internal-live and urgent Stop contexts without state publication; and unchanged authoritative-error/result-kind behavior. Worker fixtures serialize and validate their actual `Response` objects. These are deterministic software/wire-path tests, not a child-process scheduling or native-device failure reproduction.

### Follow-up identities and existing gate evidence

| Source | SHA-256 |
| --- | --- |
| `crates/omniusong-engine/src/worker.rs` | `86412c610f3d4a3d90a66001015ca587e95d034519fa5834fcc3bc803898333b` |
| `crates/omniusong-engine/src/live_supervisor.rs` | `613bae29cc64568b3472b226613c627d3e75a9c70f4e6c6f2fe03b0f3b00f9ef` |
| `crates/omniusong-engine/src/performance_session.rs` | `15ec9627d5fc61e1cb9d2cffa28af258af65950647a2140eb0b3ac6caa977524` |
| `crates/omniusong-engine/src/transport.rs` | `f8bd0e8a2b73b7019a77107de2d81d85a29d3d9d253f1ac68349e5ea3ee873f9` |

The reviewer verified these four current hashes against the coordinator's [gate-three source/command receipt](../../.runtime/rust-bounded-inspection/gate3-run.json), and verified all four referenced log hashes and zero command exit codes. The [workspace log](../../.runtime/rust-bounded-inspection/gate3-workspace.log) contains the five boundary tests passing; its 61 result groups total **811 passed, zero failed and five ignored**. Existing formatting, all-target/all-feature warnings-denied Clippy and default all-target workspace build also passed in that receipt. The reviewer read those artifacts and did not invoke Cargo or formatting. Ignored tests are not accepted checks; this gate does not constitute new packaged/native qualification.

The coordinator separately reported isolated clone costs for a 9.0027-second / 7.5 MB source (6.573 seconds for four phases) and a 17.8453-second / 14.7 MB source (12.796 seconds for four phases), with originals preserved. These measurements were not independently reproduced here. They make inspection cost a plausible contributor worth measuring against the configured ten-second sealing deadline, but are not a measurement of that failed native seal phase or proof of its cause. The diagnostic build has not been run natively. That attribution, general latency policy and further implementation remain outside this paused, in-flight review.
