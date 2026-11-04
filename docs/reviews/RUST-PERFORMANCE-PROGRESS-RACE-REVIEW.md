# Recorded-performance progress: independent race investigation

October 20, 2026. Initial read-only source investigation following the coordinator's native `a948…` recording failure, `Recorded-performance live progress regressed`; the subsequently authorized implementation is recorded separately below. No Cargo, formatter, native device/UI action or live catalog access was performed by this author. The correction subsequently passed the coordinator’s 801-test source gate; this does not establish that it caused the observed failure or accept the complete corrected native workflow.

The failed-stage receipt `.runtime/rust-bounded-inspection/failed-native-capture.json` records 1,104 complete callbacks, 565,248 durable frames, 45 batches and 46 checkpoints. Its last retained End has generation 1, applied sequence 45, no clear and no first fault. The coordinator observed a later 568,832-frame/1,111-callback live summary before retirement: seven observed callbacks and 3,584 frames beyond the selected checkpoint. That difference does not describe any additional unobserved final tail or prove a specific Panic or fault. No normal Finish or seal is established by this recording. The coordinator reports that unfocused UI input was disabled and focus cleanup can request Panic even with no held keys; this makes the race below plausible but does not prove the failure's exact cause.

## Concrete source defect

`live.rs:1068–1072` advances the callback's Panic generation and resets `control.applied` to zero before publishing its generation acknowledgment. The admission owner's `self.scope` stays in the old generation until `finish_panic` adopts the callback acknowledgment at `live.rs:779–785`. In between, `progress` reads the reset applied counter but labels it with `self.scope` (`live.rs:594–620`). It can therefore emit old-generation `Some(n) → None` while the callback correctly emitted a new-generation reset.

`PerformanceSession::snapshot` samples this actual admission progress on every call (`performance_session.rs:424–449`); its shared-status cache is not needed to produce the defect. A Status snapshot before `Runtime::update` completes pending Panic acknowledgment can expose the inconsistent old-generation snapshot. The parent's `live_supervisor.rs:271–283` correctly rejects that same-scope sequence regression. Weakening that check or globally taking maxima would conceal genuine protocol faults.

## Smallest proposed correction

On the first successful `begin_panic`, sample and retain a private old-generation applied watermark **before** publishing `requested_generation`. Pending-Panic retries retain that same value. While Panic is pending and the effective published scope still equals the admission's old scope, progress reports the frozen old watermark, old accepted sequence, Panicking state and no lease credit. This is a truthful acknowledged lower bound; it makes no claim that later old-generation batches were applied.

Keep the callback's actual zero/reset publication and its new-generation TapEnd unchanged. Only `finish_panic`, after the real generation acknowledgment, adopts the next admission scope, clears the old watermark/accepted sequence, and issues its new credit. Never combine the new scope with the old accepted sequence or old token. Existing exact first-fault selection remains authoritative: if its effective scope differs from the old admission scope, do not attach the old pinned watermark to that fault scope. No fabricated acknowledgment, timing conversion or recorded event is required.

This targeted change addresses a definite source race. Add bounded diagnostics to the existing rejection showing the old/new worker/session/generation, frame, accepted/applied watermarks and first-fault summary, plus status revisions. Keep the exact rejection condition and terminal behavior. These fixed-size observations can distinguish the actual next failure without logging raw events, paths or arbitrary payloads.

## Required deterministic regressions

1. Use the production recording renderer, apply sequence 1, request Panic, render the clear/acknowledgment callback, then sample admission progress **before** `finish_panic`. Require the old scope and `Some(1)`, valid Panicking/no credit; the actual TapEnd must already have the next scope and reset watermark. Complete the acknowledgment and require exactly the next scope, empty accepted/applied watermarks and a new credit.
2. Repeat with a full pending queue and its four-batch-per-callback discard bound. Every pre-ack snapshot must retain the old published watermark; discarded originals and callback accounting remain exact. A retry of the same Panic cannot resample a reset counter.
3. Exercise `PerformanceSession::snapshot` at that exact software-controlled boundary, then pass successive statuses through the real parent validator. Also retain a forged same-generation sequence/frame regression case that still retires/rejects with its bounded old/new diagnostic.
4. Cover an actual first fault before and after callback generation change. Its exact scope/cause remains visible, no old watermark or token leaks into a different fault scope, and fault handling cannot authorize healthy Panic or Finish.

The later execution addendum below qualifies the corrected source. Same-package native recording/Finish/attachment/playback remains a separate boundary. The prior post-recording playback-scope correction remains a separate change whose actual same-generation native acceptance was not reached in this failed attempt.

## Investigated artifacts

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `crates/omniusong-engine/src/live.rs` | 68429 | `7e5667a5c02b047a7b28624912051057b330e133a18b98667bec625765754f80` |
| `crates/omniusong-engine/src/performance_session.rs` | 77617 | `f6598f695594c5a1483ce8b9218c6f8a23520d0e4c6a3850103b1393770f596f` |
| `crates/omniusong-engine/src/live_supervisor.rs` | 57508 | `57cad49c6128957f1b89bc233464b2213afc77ec4c2d69b18b5030929db26a7b` |
| `.runtime/rust-bounded-inspection/failed-native-capture.json` | 33310 | `2e17d8213611ae9ec87b9e51ce770946ad972f3d1904c0112aae9d4b102b47f2` |

## Authorized implementation and preparation fingerprints

The coordinator adopted the narrow contract and assigned this author `live.rs`, test-only changes in `performance_session.rs`, and the parent diagnostic/regressions in `live_supervisor.rs`. `LiveAdmission::pending_panic_applied` now pins the old scope's actual applied lower bound before requesting callback clearing; retries preserve it, and only actual acknowledgment adoption clears it. The existing `same_scope` test prevents applying that pin to a different fault scope. Callback rendering, tap originals, first-fault publication and the parent rejection predicate are unchanged.

Six owner tests are source-ready: the real renderer clear-before-ACK boundary; sixteen queued discarded originals over four callbacks; old/new first-fault scope isolation; actual `PerformanceSession::snapshot` while its metadata lock is held; parent acceptance of the valid generation handoff; and rejection of genuine same-generation frame/accepted/applied regressions with unchanged cached evidence. The diagnostic contains only fixed-size identity/counter/fault values; its regression asserts a sub-2-KiB message. No paths, raw batches or variable metadata are included.

The source was frozen before coordinator formatting with these hashes: `live.rs` `475a10caa58556ecf2d16a9ed620fc151d72b79dd37ef14e1a462ef79b1fed26`; test-only session update `dfb4329a6d2ac372ce8786e6b806c26439c2522470a0901bdfb31f1088961eb6`; supervisor diagnostic/tests `9d611cc65bd4b24970e11ab4ae861baefc97ec7e6f45164cf2a880cdb2abd90a`. These are preparation fingerprints, not executed-source claims. Root and a separate reviewer own independent approval of this author's production correction. Actual native cause attribution remains bounded by the failed evidence above until a diagnosed or corrected native run is observed.


## Executed correction and subsequent native failure

The coordinator's frozen `.runtime/rust-bounded-inspection/gate2-run.json` (29,141 bytes, SHA-256 `7bf024ef8603ba11ace842229e96123543408ef73bbede78a1ae26ec06f063e4`) binds 146 source snapshots. The reviewer verified all four command-log hashes/lengths and counted **801 passed, zero failed, four intentional ignored helpers, 61 groups**. Formatting, warnings-denied all-target/all-feature Clippy and default all-target build each exited zero. This includes all six owner regressions and three independently authored public-API tests. The latter cover sixteen queued batches/four-callback drain, unchanged tap scope evidence, fault scopes and zero callback allocation/deallocation. Root independently reviewed the correction, and `RUST-PANIC-PROGRESS-REVIEW.md` records the other reviewer's acceptance.

The coordinator then observed `ffc90e34…` recording 856,576 frames / 1,673 callbacks with real keys/pads, reaching Sealing before a different visible failure: `Audio worker generation is retired; no new control admission`. The retained 14,716,928-byte journal is still Recording with no sealed manifest. This later result neither proves the earlier `a948` cause nor completes corrected native acceptance. The generic rejection hides the actual retirement trigger, so a separately approved diagnostic latch is being qualified before any further native attempt. Its exact cause must be measured; no timeout or seal-authority change is justified by the generic message alone. The prior same-generation playback path remains unqualified on the corrected package.
