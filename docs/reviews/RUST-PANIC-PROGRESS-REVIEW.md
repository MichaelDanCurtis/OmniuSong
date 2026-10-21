# Independent Panic progress review

October 20, 2026. Source review of the architect's frozen correction and its coordinator formatting. **Accepted at the source-review boundary: no blocking finding in this narrow diff.** This review ran no Cargo, formatter, native device/UI action, package operation or catalog mutation. Executable gates and a corrected-package native walkthrough remain separate acceptance requirements.

The coordinator's `a948…` native attempt failed with `Recorded-performance live progress regressed` before Finish. No intentional Panic was reported. Focus cleanup can request Panic, but the retained durable prefix has no clear identifying this as the actual cause. The definite source race and that attribution limit are recorded in [the preceding investigation](RUST-PERFORMANCE-PROGRESS-RACE-REVIEW.md). This correction does not turn the failed attempt into a normal Finish, sealed take, or accepted native recording.

## Ordering and scope

In [`LiveAdmission`](../../crates/omniusong-engine/src/live.rs), the new `pending_panic_applied: Option<u64>` belongs to the sole off-callback admission owner. On the first successful `begin_panic`, an Acquire load samples the actual `control.applied` watermark **before** the Release publication of `requested_generation`. `pending_panic`, removal of the old lease token and the pin are installed before that publication. The existing pending-request branch returns before sampling again, so an identical retry cannot replace the pin with the callback's later reset zero.

`progress` selects the effective scope from the exact first fault when present, otherwise from the admission's current scope. Its existing `same_scope` check encloses both accepted and applied watermarks. Therefore a pending healthy Panic reports the old scope, old accepted value, frozen observed applied lower bound, Panicking state and no credit. If a real first fault belongs to a different generation, neither old watermark is relabelled with that fault scope. The one-shot fault identity and cause remain unchanged. No maximum, inferred generation, or guessed scope repairs a snapshot.

Only `finish_panic`, after acquiring the actual matching `applied_generation` acknowledgment, adopts the next scope, clears the pending pin and accepted sequence, and creates the new scope's credit. Faulted and Finish-closed admission remain rejected. New-generation accepted/applied values start empty; the old token is not reused. An observed lower bound is not a claim that later queued old-generation batches played.

The renderer's Panic branch is unchanged: it clears voices, disposes at most four queued batches per callback, resets its real applied counter, and publishes its new generation. Its recorded `TapEnd` remains callback truth. The off-callback pin never changes a recorded End, clear, batch disposition or original event. In particular, the public old-generation pending snapshot may retain `Some(1)` while the actual new-generation End correctly records `None`.

## Realtime and parent boundaries

The production correction adds a scalar field and off-callback atomic observation. It adds no callback allocation, deallocation, lock, I/O, queue capacity or unbounded loop. The existing callback's bounded drain and tap logic are untouched. Token generation remains in the pre-existing off-callback adoption path.

[`PerformanceSession::snapshot`](../../crates/omniusong-engine/src/performance_session.rs) still samples real admission progress; its production behavior is unchanged by this patch. The new session regression also holds the background status lock, exercising the cached-metadata fallback at the callback-ACK/admission-adoption boundary.

The rejection predicate in [`live_supervisor.rs`](../../crates/omniusong-engine/src/live_supervisor.rs) is unchanged. Genuine same-scope accepted/applied regression, decreasing frames, invalid scope transitions and changed first faults still fail closed. The diagnostic adds fixed-size old/new revisions, scopes, states, counters and fault summaries. It contains no raw event vector, source path or arbitrary device text. Parent tests retain the prior accepted snapshot after forged regressions; no new Ready is exposed by accepting a contradictory observation.

## Regression evidence and remaining gate

The architect added six source regressions: three admission/tap cases, one session snapshot case and two parent-validation cases. They cover the real reset boundary, four callbacks draining sixteen queued batches, repeated request identity, faults on either side of generation change, fresh credit adoption, and rejection of genuine regressions.

The independent [`panic_progress_review.rs`](../../crates/omniusong-engine/tests/panic_progress_review.rs) adds three public-API tests. They use production software renderers and check old-scope pinning through the full bounded queue drain; truthful recorded new-generation End versus pending public progress; and exact fault scope before/after acknowledgment. The callback invocations are instrumented to require zero allocations **and** zero deallocations. These test sources were not executed by this reviewer; the coordinator owns their results.

Before closing native qualification, run those tests with the existing realtime, tap, Finish and parent guards, then repeat the actual same-package output-only Record → Ready → keys/pads/focus cleanup → Finish → seal → explicit attachment → playback flow. Retain the new bounded old/new diagnostic if it fails again. A source correction and deterministic software reproduction establish neither this failed native attempt's exact cause nor physical output alignment, callback lifetime acceptance, native input, Windows, or the full milestone.

## Reviewed identities

The architect's announced source was checked before coordinator formatting; the following hashes were then read from the formatted files during this review. Formatting is not an executable test result.

| File | SHA-256 |
| --- | --- |
| `crates/omniusong-engine/src/live.rs` | `93b76db41f703ac9ac4e626488f20805b18cc13e7565a5f1ab966cc6edfe9bd9` |
| `crates/omniusong-engine/src/performance_session.rs` | `dfb4329a6d2ac372ce8786e6b806c26439c2522470a0901bdfb31f1088961eb6` |
| `crates/omniusong-engine/src/live_supervisor.rs` | `9d611cc65bd4b24970e11ab4ae861baefc97ec7e6f45164cf2a880cdb2abd90a` |
| `crates/omniusong-engine/tests/panic_progress_review.rs` | `468d3744bf6ad38a0b67e526905db850ff784e4ada5e33310399b823950c28ae` |
