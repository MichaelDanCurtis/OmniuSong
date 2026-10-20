# Initial live audition implementation review

Status: accepted for this bounded software slice. No unresolved blocking source finding remains; the final unified software gate passed. Native live UI/device acceptance remains pending. This review covers the first procedural, output-only live slice against `docs/contracts/NATIVE-LIVE-AUDITION.md`. It does not qualify input capture, controller delivery, MIDI 2 transport/MPE, native callback timing, audible quality or physical latency. The reviewer ran no Cargo commands and opened no devices or UI.

## Concrete findings and disposition

| Finding | Source correction / acceptance condition |
| --- | --- |
| A token admitted exactly when `frames == deadline` could extend a lease before the next callback sampled that endpoint. | `live.rs:116` (`renew`) rejects the expired endpoint; the callback now arbitrates expiry before publishing its end-frame count. An independent exact-deadline test requires sticky `LeaseExpired`, unchanged deadline and silence. |
| Sustain-delayed NoteOff discarded its 16-bit release velocity. | `live.rs:645` retains the pending release velocity; pedal-up uses that value. Independent repeated-pitch/source-sustain tests require fast maximum-velocity release, with another source unaffected. Pulse has its own retained automatic-release velocity and independent regression. |
| Panic generation exhaustion returned an error without latching a fault. | `live.rs:490` (`begin_panic`) now latches `CounterExhausted`; the independent maximum-generation test forbids a healthy reusable scope or credit. |
| Separate fault-state reads and separate active/tail count atomics could construct an invalid otherwise healthy progress DTO. | `live.rs:364` reads fault publication through one captured state; voice/tail counts are packed in one atomic value. Writing-fault state cannot appear healthy. Source audit is required in addition to offline allocation tests; these tests do not exhaustively model every thread interleaving. |
| Internal LiveBatch/Panic errors could disappear behind later successful telemetry. Same-scope progress and first-fault replacement were insufficiently checked. | `live_supervisor.rs:487` now requires authenticated sticky fault evidence for a known live rejection, otherwise retires with the exact error. It rejects regressing progress, replaced first fault, unsolicited session changes and missing authoritative live updates. Private-process tests cover exact error retention behind an ordinary completion and contradictory authenticated updates. |
| A completed Stop could reopen parent admission until the UI consumed its stopped update. | `transport.rs:491` and `:539` check `Shared.live_active`, which gates admission independently of UI polling; reliable Stop closes it immediately, and both ordinary/reliable Stop keep it closed. Private-process tests withhold UI update polling and verify rejection without retiring a healthy stopped peer. |
| LiveStarted handling cleared the shared fault latch after publishing a newer callback fault. | Corrected in `live_supervisor.rs:43`: clear the old-session latch before applying the new authoritative update; preserve the newly reported fault. The independent helper returns a valid Running Ready followed by a newer Device fault in the same response; parent admission must reject before any UI update poll. |

Healthy Panic also retains the per-source GUI tick high-watermarks for the unchanged clock epoch (`live.rs:515` and `:672`), while resetting voice/note/sustain state. The added independent regression rejects an older timestamp in the new panic scope.

The private-helper tests are actual child processes using the inherited private socket and current build/profile handshake. Their audio work uses the real CPU-only live renderer. Those acknowledgments test protocol/fence handling; they are not evidence that a native CoreAudio stream or error closure has been destroyed.

## Bounded callback and control audit

`live_pair` prepares tables, queue and ownership before rendering. Only `LiveRenderer` mutates voices, sustain and applied sequences. A whole packet stages fixed source/voice arrays before commit; a later invalid event cannot produce a partially sounded packet. The 64-slot pool includes release tails; exhaustion faults instead of stealing a held voice. The source/connection identity and per-source issued handles prevent another source's release or old scope from releasing a current voice.

The callback drains four packets total per native invocation, including empty packets and discarded panic-generation backlog. Splitting output into 2,048-frame sub-blocks does not increase this budget. Independent cases cover 1, 2,047, 2,048, 2,049 and 16,384 frames, queue refill, malformed channel shape and oversize, all three output conversions, 64 voices and all procedural patches. Thread-local allocator instrumentation checks allocations and deallocations during rendering and callback-owner destruction while control owners remain alive.

Lease rotation occurs on admission; queued packets retain their original token. Empty GUI batches consume the same sequence stream as musical packets. Status reads and old-token backlog do not renew. Panic acknowledges only after callback clearing and bounded old-queue disposal; generation/sequence reset requires the matching callback result. Faulted sessions require Stop/new Start.

The supervisor has bounded independent ingress and urgent obligations but one serialized private socket. Its ordinary reply outbox must remain nonblocking while the child exists. Process tests hold a normal Status completion while Panic is applied, and fence a Start already in flight with reliable Stop. The cumulative update channel and ordinary/terminal reply channel have distinct consumers. `is_retired()` reports supervisor retirement independently of whether a terminal error carries the pending request ID or zero.

Native `LiveOutput::stop` pauses successfully, drops the stream, then waits for both renderer and error-closure destruction acknowledgments while retaining backing owners. Failed pause or unknown quiescence retains unsafe-to-release ownership and retires the process before any new session. Parent loss still has the independent worker watchdog. This is source-level review, not a claim that every driver behaves correctly or that exceptional backend error paths are universally realtime-safe.

## Read-only UI audit

No new blocking UI finding was identified in `live_ui.rs` / `audio_ui.rs`. Local note maps are bounded; separate input admission remains usable while ordinary control is busy. GUI credit is removed on successful parent admission and replaced only by a newer matching-scope issuance watermark. Panic rearm requires the exact ticket, successor scope, Running progress, zero voices, sequence one and credit. Fault-first cumulative telemetry is reapplied after a delayed ordinary Ready; Stop during Start prevents late Ready from reopening note routing.

Focus loss, PointerGone, leaving the pointer surface, text focus, popup/IME input and mode/octave changes clear local input and request cleanup; computer input requires explicit enabling again. Pointer release is not also an accessibility pulse. Accessibility activation uses callback-owned bounded Pulse. Source-owned sustain and repeated physical-origin suppression are explicit. The app advertises procedural/output-only/not-recorded status and negotiated output configuration. Live guards block session changes; typed worker retirement releases only the output guard while capture uncertainty/originals remain retained.

These observations support bounded software acceptance after the fresh central tests pass. Actual keyboard/pad/focus behavior, hearing the result, native teardown and all original-file preservation still need the coordinator's separate UI/device evidence.

## Evidence status

Owned independent sources: `engine/tests/live_review.rs` (19 active), `engine/tests/live_process_review.rs` (7 active plus one ignored subprocess entrypoint), and the existing `engine_process_review.rs` (five original active cases updated only for current build/profile/Initialize/update shape).

The first central engine run passed 22 owner tests, the five existing process tests and 18 existing engine-review tests. Its new process binary passed four cases and failed the no-update-poll Stop case because the helper omitted its intended `after-stop` marker; the preserved `worker.status` and `worker.stop` artifacts and source inspection confirmed this was fixture instrumentation. The marker is now written only after an actual subsequent Status request, while the helper already holds the completed Stop receipt. The original assertion remains. This is not a production-failure claim and is not a pass for the corrected test. Log: `.runtime/rust-capture-recovery/live-engine-initial-tests.log`.

The next central workspace run passed **328 tests, zero failures, two ignored subprocess entrypoints, 37 result groups**. Its seven live-process cases include all corrected Stop, internal error, progress regression and start/fault-ordering regressions. Its renderer-review binary contained 18 tests; the final newly saved same-epoch clock regression was subsequently included in `.runtime/rust-capture-recovery/live-render-review-final-tests.log`, which passed **19/19**. Both logs were read directly. The final unified post-formatting gate, `.runtime/rust-capture-recovery/live-workspace-integrated-tests.log`, passed **329 tests, zero failures, two ignored helper entrypoints, 37 result groups**, including all 19 live-renderer review cases, seven live-process cases, five existing process cases, 27 engine owner cases and 47 app cases. Clippy with all targets/features and warnings denied passed (`live-clippy-final-2.log`); format check passed (`live-fmt-final.log`). The reviewer read these artifacts directly. No earlier 278-test workspace result qualifies these sources.

Reviewed source hashes after the style-only fixes:

| File (relative to `crates/`) | SHA-256 |
| --- | --- |
| `omniusong-engine/src/live_types.rs` | `df6d4eacb411325a4f5000728c3938388b23726c62c7c8897ef3d5bd333c27d2` |
| `omniusong-engine/src/live.rs` | `158862997cc3d32467cf25e82bcc54eb33ebe47e2ac14eb64792f2e48741afa3` |
| `omniusong-engine/src/live_supervisor.rs` | `6c57af374bf0d6c92da88e0a655805b695cfa0effa41f20abd896586a9ae26aa` |
| `omniusong-engine/src/transport.rs` | `9e2d99252c4e5ac655b9db10cf1a01103ad83a086dc9585bd3dd235b3df5f5d4` |
| `omniusong-engine/src/worker.rs` | `fefe2685a13cfbb9bef23a8d21804fb50ab9d9a0ef7005a510f85cf59ae7fdeb` |
| `omniusong-engine/tests/live_review.rs` | `72ea33e7311259026117a8e238c1e13295c0dea1cfe47fc5431f894e2c16b224` |
| `omniusong-engine/tests/live_process_review.rs` | `a6d2475fc5b29ac7f81b3c1dd5247d60d3f91da74dfdc0dd60f46d240daf13e0` |
| `omniusong-engine/tests/engine_process_review.rs` | `212257da0cbe63dcbe87d46570a9f33dd251dbbe14358d3eda84de5ee4004036` |
| `omniusong-app/src/live_ui.rs` | `90d819fd2ec16b5cdd082b2c56378cc5ac2b380c89ab3361793937f3a8fc8ed9` |
| `omniusong-app/src/audio_ui.rs` | `dc76bc129d83da3f274892a004ff4b204964cece4530c6cfff9dfd88b41f02a2` |

The 328-test workspace log SHA-256 is `860ecc2d46cbe43c06f448045adf952818a4c565b2b2ae32e1e310aa6791a3c2`; the supplemental 19-test renderer log is `9b3cc8ad9bbae816ca237b8cee4d33ddd2ab6a8542ff0fcf86910760f9ad2fb2`. Native device/UI observations belong in the coordinator's separate verification receipt, not in these software counts.

Final integrated log SHA-256: `3d133fb9584471ae322dfab479bb0bc40effc925fd18bc0d31e0b206b49f9295`; Clippy log: `531526d59fc8edee4b672171772d50fd7373621da68f07ccff7940c3d3c9fa3b`; empty successful format log: `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.

The coordinator's final offline receipt, `docs/verification/rust-native-live-bus.json`, reports 108,000 stereo frames at 48 kHz, 18 accepted batches, zero transfer gaps and exact attachment/Undo/Redo/archive restoration. PCM SHA-256 is `6b4e3f00d76c9cdbbf17bb0cd72a93c3481a965aa64158b73568e46345ad9d20`; the 1,028,096-byte archive SHA-256 is `04f52ea65f4665b7c4717cf85489d2ea3ddc2467f8d10675444d414209f4b723`. This is explicitly synthetic: no device, human performance, elapsed capture, native UI delivery or physical latency. Build/signature success and bundle SHA-256 `35822214bdc1e3b32ba019a2ad7ec9e32fe0c957bb507b4cbdd5eb1f217bf1e6` are coordinator-reported packaging evidence; the current app is still loading its external-drive store. No native-live/output acceptance is asserted here.
