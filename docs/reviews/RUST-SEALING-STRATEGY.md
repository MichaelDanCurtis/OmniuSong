# Recording seal strategy and long-session bounds

October 21, 2026. Architecture review against `codex/native-recording-foundation`, source `e95d5e955611fa6a141ee6588236f11869860c43`. This document proposes interfaces and acceptance gates; it does not change production code, authorize recovery, qualify a new binary, or change a deadline. The coordinator owns packaging, Cargo commands, the native process, and actual output-only qualification. No native input is needed.

## Diagnosis and evidence boundary

The `ffc90e34…` native recording retained 856,576 frames, 1,673 callbacks and a 14,716,928-byte journal, then retired during sealing. At 48 kHz that frame count is 17.845333 seconds. The original has no manifest or attachment and retains its rollback journal. Its immutable source and sidecar hashes, the generic original failure, and the limitations of the isolated clone profile remain in [RUST-RECORDING-SEAL-COST](RUST-RECORDING-SEAL-COST.md). Do not repair, open for writing, clean up, replay or normally seal this failed stage to investigate a new implementation.

The clone's 12.795799-second four-phase cost exceeds the unchanged ten-second seal deadline; this establishes an unsuitable cost/deadline combination for that measured debug workload. It does **not** prove the reason the original eighteen-second native run retired.

During this review the coordinator packaged the corrected diagnostic source as binary `cbb6cae38a914f409df8af37b6dddff706e940a59cf8b7bab6de9e0f9898ed11` and ran a fresh output-only capture. The [native receipt](../../.runtime/rust-seal-fix/diagnostic-native.json), recorded October 22 UTC / October 21 local, reports **1,892,864 frames / 3,697 callbacks / 39.434667 seconds at 48 kHz stereo** followed by the exact visible cause: “Recorded output sealing deadline expired; worker retired and original retained.” This confirms the sealing deadline as the cause for **this fresh run only**. The prior eighteen-second failure is consistent with it but remains retrospectively unproven.

The fresh stage `54ceabeb8390487bbe9b83cd7e58d509` retains its 32,436,224-byte `take.sqlite3`, SHA-256 `9606c000cce047980fea6df8af8006d29ea41a45d750e6c60ee1fb79a2e0668e`, and owner lock; this receipt lists no rollback sidecar. Sidecar absence does not identify which internal sealing substep was executing. Intent revisions retain Start, exact Finish, then explicit release/Keep locally; no normal seal or attachment succeeded, and the coordinator verified app exit. The old failed stage and its rollback journal remain separate preserved evidence.

The relevant current path is:

1. The concrete I/O owner observes the actual callback Finish, stops the stream and proves real closure/producer destruction. Only `performance_session::native` constructs `NativeSealPermit`.
2. `writer_run` drains the tap and starts the `SEALING` watch **before** calling `PerformanceWriter::seal_native` (`performance_session.rs`, `writer_run`). The watch covers both preparation and commit, and does not reset on same-phase retry.
3. `seal_native` verifies exact in-memory Finish/cut, healthy durable counters, no pending callback/group/write/error, then performs an owned inspection, an independent semantic pass and `Journal::complete_prefix` (`performance_writer.rs`, `seal_native`). It retains the complete receipt and deterministic operation ID before attempting finalization.
4. `permit.authorize_seal()` rechecks actual lifetime/Finish evidence and competes with cancellation through `ACTIVE -> SEAL_AUTHORIZED`. Its place after semantic review matters: cancellation during preparation can still prevent normal seal.
5. `seal_complete_prefix` authenticates and finalizes under one SQLite Immediate transaction. It verifies original rows again after its own writes, then commits. A later Stop cannot promise that no manifest exists. Exact retry authenticates the same originals and returns the same committed manifest.

The measured costs and current source both implicate redundant work as a concrete optimization target. For a fresh normal `Finalization::Complete` on a Recording journal, the call graph in `journal.rs::finish`, `complete_snapshot` and `verify_sealed` executes:

| Work inside finalization | Full chunk scans | Original-row hashes |
| --- | ---: | ---: |
| Prewrite `complete_snapshot` | 1 | 1 |
| Separate admitted-original comparison | 0 | 1 |
| Report used to build manifest | 1 | 0 |
| Postwrite admitted-original comparison | 0 | 1 |
| Postwrite `complete_snapshot`, including normal `verify_sealed` | 3 | 1 |
| Final explicit `verify_sealed` comparison | 2 | 0 |
| **Total in one fresh finalization** | **7** | **4** |

There are also two complete generic scans and two original-row hashes before finalization (owned inspection and `complete_prefix`), a physical-file hash, and a semantic pass which reloads/authenticates every selected chunk. These are source call counts for this path, not profiler samples and not the retry-path count.

`inspection.rs::hash_field` prepares SQL for field type/length, scalar values and each bounded 64 KiB piece. `original_rows_hash_bounded` invokes it for every original column in every original row. `journal.rs::load_chunk_bounded` separately prepares three repeated queries per chunk, and semantic `read_ordinal` adds a fourth lookup. SQL preparation and repeated full scans should be measured independently; no measurement yet assigns a percentage of runtime to either.

## Invariants that every optimization must preserve

- Keep every original byte and row, including opaque extension columns, SQL storage types, row identity, schema and sequence metadata. Preserve the existing domain-separated original-row digest encoding and chunk-index encoding. Cached chunk hashes, file modification times, the writer's counters or a checkpoint alone do not authenticate a stored original.
- Retain strict v2 ownership; exact descriptor, complete physical chunk count, all six stream prefixes, semantic Finish/End/checkpoint, no gaps, and no damaged/unselected tail. A shorter valid prefix cannot become a normal completion.
- Keep the concrete nonclone native permit and the last cancellation CAS. Public progress, a storage receipt, a valid manifest on an isolated clone, or software test observations cannot mint native authority. No constructor is widened and no diagnostic helper fabricates a permit.
- Keep one exclusive owner and one finalization transaction. Recheck original identity after preparation and after manifest/state writes; hostile triggers and unknown original fields remain covered. Preserve exact normal replay and reject Interrupted/recovered promotion or a changed operation.
- Before authorization, cancellation wins or loses exactly once. After authorization or an unacknowledged commit, cancellation/retirement is an abandonment decision with a possibly committed result. Retain exact operation, descriptor, cut, receipt and stage. Never relabel it definitely unsealed without positive evidence.
- Keep immutable first retirement cause, response/result correlation, the response fence, source identity and terminal error publication. Parent traffic or repeated Finish/Stop/Status must not extend a deadline.
- A stuck OS/filesystem call cannot safely be killed by dropping its Rust thread. Keep callback backing and the bounded owner alive as required, retire the generation and do not admit another session under unproven ownership. A timed-out transaction is not a successful Finish or proof of rollback.

## Implementation sequence

### A. First slice: preserve all checks and reduce SQL preparation

The coordinator has selected a narrow first slice: reuse prepared statements in the original-field hashing loop in `inspection.rs` only. Use statements scoped to one table/column traversal; never create a cache entry per row/value. Preserve the length-first read, 64 KiB piece cap, exact type semantics, original byte encoding, all inspection charges/checkpoints and sticky stop errors. Do not replace bounded field reads with a whole-BLOB allocation. Do not change the query's coercion semantics while making this change. Repeated chunk SQL in `journal.rs` remains a later independent optimization candidate.

This slice leaves all scan counts, the native permit, IPC, watchdog constants and on-disk schema unchanged. Its acceptance is equality of receipts/rejection behavior and a repeated timing comparison on fresh isolated clones. A source test pass alone is not a performance result. This is the smallest useful implementation to agree first.

Use a preserved baseline and fresh clone/receipt for every measurement, with explicit source and output paths. Record build profile and source/binary identity, four-phase durations, rows/chunks/payload/physical bytes, memory, original hashes and directory names. Retain each outcome; do not overwrite earlier receipts. Use small, comparable-long and larger synthetic fixtures, with repeated runs sufficient to show variation. Keep debug and packaged-profile results distinct. All baseline comparisons must use the same workload and machine conditions; copying/preflight may remain excluded only if separately reported.

### B. Second slice: transaction-scoped validation facts

If the first slice is insufficient for the supported recording envelope, remove repeated work by sharing already verified facts **inside one held transaction and one read/write epoch**. Introduce a private recording-crate value conceptually named `CompleteFacts`, containing the current validated descriptor, complete `RecoveryReport`, exact physical counts, original-row digest and checked mode/derived-metadata state. Its fields and constructor stay private; it is neither serialized nor supplied by the engine. It carries no native lifecycle authority.

For a fresh normal finalization:

1. Begin Immediate; validate descriptor/v2/current state, zero recovery metadata, physical stream/chunk counts, one full chunk scan and one full original-row hash. Compare the resulting public `CompletePrefix` to the admitted receipt.
2. Build the manifest from those exact facts. Insert manifest and update state using the existing transaction boundaries.
3. Recompute a separate full postwrite scan and original-row hash. Explicitly verify current descriptor, Sealed state, zero recovery metadata, physical counts, all prefixes/index/payload totals and the stored manifest's canonical bytes/hash/operation/normal mode against the proposed manifest. Compare the postwrite complete receipt to the admitted receipt.
4. Run the existing before-commit hook, commit, then run the after-commit hook. Keep rollback versus committed-but-unacknowledged tests.

The proposed target is **two full scans and two all-original hashes inside fresh finalization**, not zero. Reusing the prewrite facts after writes is forbidden: current tests deliberately install triggers that add recovery metadata or reverse state. Separating a bounded stored-manifest reader from its scan-based verifier allows the postwrite checks to consume the postwrite report without recursively rescanning. The public `verify_sealed` and recovery semantics remain independently correct.

The already-Sealed replay path should use one newly authenticated normal snapshot and a strict stored-manifest comparison, then return the original manifest with no writes. It must still reject changed originals, operation, mode or descriptor. Do not optimize replay into trusting a previously cached success.

An optional follow-on lets an `OwnedJournalView` derive a complete receipt from its same held, already authenticated inspection plus explicit strict-v2/count/no-gap/no-recovery checks. It may remove the separate preauthorization `complete_prefix` scan/hash. It must not create a complete receipt from an arbitrary `JournalInspection`, recovered prefix or caller struct. The semantic pass remains and the final transaction still reauthenticates the admitted receipt.

Avoid combining semantic parsing and storage validation in the first change. A streaming visitor could eventually validate the same authenticated chunks without reloading them, but it crosses the recording/engine boundary and requires a separate interface review. No incremental writer cache replaces final stored-byte authentication.

### C. Separate slice: bounded seal progress and abandonment

Only after representative optimized measurements, select a sealing policy for the supported envelope. Keep starting, Finish, native-stop and draining deadlines unchanged. Do not replace ten seconds with a broad larger constant based on two debug samples.

The future sealing policy needs three independently enforced limits:

- **Cumulative work caps:** bytes, original/visited rows, chunks, records, groups and SQL VM steps. Compute conservative limits from a finite supported capture envelope and a finite pass plan. Charge repeated work; a retry does not receive a fresh budget.
- **No-progress deadline:** advances only after an actual bounded unit of storage/semantic work completes. A UI poll, loop iteration, SQL-start event, heartbeat, phase label or retry is not progress. Count raw field pieces only after successful read/hash; count semantic groups only after full validation. SQL VM steps can enforce a cap but are not by themselves proof of useful forward progress.
- **Absolute operation deadline:** selected once from admitted bounded work and a measured conservative throughput floor plus fixed setup/commit allowance, with a compiled maximum. It never extends. Progress does not keep an operation alive forever. The sizing decision cannot depend on unauthenticated/unbounded declared counts; for a new owned journal it uses checked append accounting capped by the supported envelope and cross-checks the actual final counts.

Conceptual private engine interface: `SealWorkControl` owns a sticky stop flag, immutable operation start/hard deadline, finite phase plan and monotonic work snapshot. Recording/semantic routines accept only an observer/governor, never a native permit or an authorization bool. A future wire `SealProgress` may expose current step, completed/total work and elapsed time; it remains observation only. Exact names and wire-version changes belong to the coordinator's interface agreement before coding.

Use `ReviewOriginals -> ReviewSemantics -> AuthenticateBeforeCommit -> WriteManifest -> AuthenticateAfterWrite -> Commit -> Published` as a bounded internal work plan. Steps have fixed ordinals and cumulative counts, so an operation cannot keep resetting a per-step watchdog. The watchdog claims a generation/operation plus observed step/progress token atomically; a stale timer must not retire a different operation or a step that made progress before the expiration claim. Keep the existing lifecycle watch's monotonic/compare-exchange properties, with deterministic tests for the new token. Do not try to encode additional steps into the current three-bit lifecycle phase field without redesigning that representation.

The following is an implementable watch protocol for interface review:

1. Allocate a private `Arc<SealWatch>` once per operation, retaining immutable worker/start/Finish identity, start instant, absolute deadline and admitted work limits. Never reuse this object for a new operation or reset its start on retry. A bounded attempt index plus step index orders retry work monotonically; all attempts consume the original cumulative work allowance. Each `PerformanceSession` has only one such operation; arming it must preserve any cancellation already won during capture.
2. Use one `AtomicU64` shared **admission and watch word**, extending/replacing the current separate `AtomicU8` gate in this later governor slice. Reserve flags for `CANCELLED`, `AUTHORIZED`, `EXPIRED` and `COMPLETED`, plus a packed elapsed-millisecond timestamp and bounded step/attempt ordinal. The timestamp is the last **completed** useful-work checkpoint. Use checked packing with a compiled maximum; arithmetic exhaustion fails closed. Detailed byte/row/group counters may live in independent monotonic atomics for observation, but watchdog/admission decisions use only this coherent word.
3. Only the real off-callback scanner/semantic consumer publishes progress, after completing and charging a bounded work unit. A compare-exchange loop updates the timestamp/ordinal only while neither terminal bit is set. Step entry alone preserves the old timestamp. Progress in the same millisecond need not change the word; that cannot affect a multi-second idle limit. The finite step plan and work caps prevent made-up resets.
4. Idle expiry reads a word, computes its step-specific idle deadline, and compare-exchanges **that exact word** to the same word with `EXPIRED`. If progress/completion raced first, the CAS fails and the watchdog reevaluates. On success it invokes sticky cancellation and immutable retirement publication. No status mutex is held during this claim.
5. Absolute expiry uses an atomic `fetch_or(EXPIRED)` so continued progress cannot starve its claim. If the old word already had `COMPLETED`, no retirement occurs; if it already had `EXPIRED`, another path owns the immutable cause. Otherwise retire. Progress CAS can never overwrite a terminal bit. A terminal word may contain both bits after a completion/expiry race; it must never be reset to active.
6. Publish a confirmed committed manifest/status before marking `COMPLETED`. If expiry already won, this does not clear retirement or revive the worker. The retained known result may become available for reconciliation; the response fence still returns the latched terminal cause. If completion won first, a later expiry cannot retire this completed operation. These two linearization orders require explicit tests.

The watch payload is a liveness mechanism, not the source of native seal authority. Only the existing private permit method may set `AUTHORIZED`, after its real closure/cut/health checks. It CASes the exact observed word only when neither `CANCELLED` nor `EXPIRED` is set, preserving watch fields. Cancellation atomically sets `CANCELLED` and uses the previous `AUTHORIZED` bit to determine whether it won before authorization; it never clears `AUTHORIZED`. Expiry atomically sets `EXPIRED` in the same word and likewise observes whether authorization already won. Thus an expiry claim prevents any later authorization CAS without a two-atomic scheduling window. If authorization won first, expiration reports a possibly committed result. The private permit's exact committed-retry rules remain separate from permission to start more work after expiration.

Expiry also propagates to cooperative scan/SQL cancellation and immutable retirement publication. Progress CAS preserves authorization/cancellation flags and cannot overwrite a terminal state; it must stop when cancellation/expiry is observed. `COMPLETED` is only a known-result liveness marker and never grants authority. This proposed synchronization changes an important race boundary: independent review and deterministic cancellation/authorization/expiry tests are mandatory before adopting it. Do not approximate it with a separate expired boolean checked before the old gate CAS.

Choose the immutable hard budget from a conservative, measured upper cost model such as `fixed + a * authenticated_bytes + b * row_visits + c * semantic_records + d * groups + commit_allowance`, with nonnegative coefficients validated on the supported envelope. Keep a separate compiled maximum. If the required budget or work exceeds that maximum, reject this normal-seal admission and retain the stage; do not silently clamp the required time and present the recording as supported. Rates/coefficients and idle/commit limits are intentionally not guessed from the existing two debug samples. Their exact values are an acceptance output of profiling, accompanied by a source and build identity.

Progress publication stays off callbacks and outside SQL/status locks, using bounded atomic snapshots or a bounded nonblocking observer. SQL progress/cancellation hooks must not perform IPC, log arbitrary strings, allocate, or take the status mutex. Their errors must remain typed sticky stop errors rather than a corrupt-tail diagnostic. Reuse the bounded-inspection lessons without globally loosening its untrusted-reader ceilings.

Before the permit CAS, Stop/deadline stops review at a bounded checkpoint and leaves normal seal unauthorized. If a read is stuck, watchdog retirement still happens independently; preserve the owner. After the CAS, stopping computation can request transaction interruption/rollback at cooperative boundaries, but the public result remains uncertain until a definite rollback/commit outcome is observed. A stuck commit has a finite absolute/commit deadline and no synthetic progress; retain the journal and possible rollback sidecar for later explicit read-only reconciliation. Never remove a sidecar to make reconciliation succeed.

Cancellation cannot erase retained intent or turn a worker-retirement reason into a newer generic error. Late success after retirement may be retained for reconciliation but must not revive the worker, republish Ready or imply permission for a new recording. A manifest known committed before retirement remains known committed.

## Thirty-minute qualification is a separate capacity problem

The goal's thirty-minute gate is actual device continuity plus a complete Finish/seal/edit/restart/archive workflow. It is not satisfied by increasing a timer or by a thirty-minute synthetic render.

At 48 kHz stereo F32, thirty minutes contains **86,400,000 frames and 691,200,000 PCM bytes (659.18 MiB)** before headers, event records, checkpoints or SQLite overhead. A 512-frame callback produces 168,750 callbacks. At the observed longer fixture's density, linear sizing would be about 345,166 chunks and 1.484 GB of journal bytes. These latter figures are capacity illustrations, not maxima or measured thirty-minute results.

The current retained-inspection ceilings are 512 MiB physical file, 262,144 physical rows, 1 GiB cumulative work and 524,288 chunk visits (`recording/src/budget.rs`). Even the raw PCM floor exceeds the physical-file ceiling. The observed-density row estimate also exceeds the physical-row ceiling; repeated passes can exhaust the visit/work ceilings earlier. Current native seal does not use this governor, so applying its defaults to sealing would introduce a separate deterministic capacity failure.

Duration and sample rate alone cannot bound cost. The supported descriptors permit up to 768 kHz and eight tap channels, callbacks may be tiny, events can be dense, and opaque original fields are authenticated. A 30-minute promise therefore needs an explicit, tested envelope of sample rate/channels, callback/event density, bytes/rows/records and storage conditions. Identify the actual qualification rig and observed buffer behavior. Compute independent worst-case limits with checked arithmetic; if a runtime limit is reached, retain the original and provide a bounded interruption or explicitly defined finish behavior, never discard packets or manufacture a normal cut. A short duration must not bypass byte/row/work limits.

Use a dedicated owned-seal budget, separately reviewed from untrusted retained-source inspection. Increasing retained-reader capacity is another contract change requiring proven cumulative work and cancellation bounds. Attachment, owned verification, archive/copy, waveform and playback preparation must also handle the supported long take; the existing short playback preparation cap does not establish that. Keep all these gates visible in the release matrix.

A viable initial target is linear storage work with bounded resident memory and a fixed small number of full authentication passes. The current cost extrapolated to thirty minutes would be approximately 1,291 seconds only under an unjustified constant-rate assumption; do not use that extrapolation as a watchdog value. Benchmark actual larger generated journals and then the real thirty-minute output path. If even the reduced complete passes cannot meet a usable Finish bound, redesign incremental storage/authentication under a new reviewed format instead of silently trusting in-memory hashes or weakening authentication.

## Acceptance and integration boundaries

The first implementation agreement covers only per-column statement reuse in `omniusong-recording/src/inspection.rs` and its measurement helper; no native session/API/deadline change. The coordinator owns integration and commands. A separate persistence reviewer compares before/after acceptance and failure semantics.

For transaction-fact work, retain existing independent checks and add focused cases proving:

- stale descriptor and rehashed PCM changes, same-prefix/different original types, sequence metadata, opaque columns and extra physical rows still reject;
- trigger changes to originals, descriptor, prefixes, recovery metadata, stored manifest or state between prewrite and postwrite checks roll back atomically;
- normal versus recovered identity and exact operation replay, before-commit failure and lost postcommit acknowledgement remain distinct;
- optimized receipts match the old digest encoding on representative extension/type/boundary fixtures;
- scans/hash invocations are bounded by the planned pass count without replacing semantic checks with counters.

For a later governor, independently test no-progress expiry despite continuing heartbeats, fake progress rejection, absolute cap despite real incremental progress, capped retries, stop before/after authorization, race with phase advancement, stalled SQL/commit, sticky cancellation, late success/terminal publication and owner/credit retention. Deterministic clocks and injected hooks are software evidence; no private native permit fabrication is needed.

The diagnostic native failure is now observed. Fixed-build native acceptance remains a fresh comparable output-only recording, exact Finish/cut and normal manifest, explicit attachment, same-generation Play/Status/Stop, Undo/Redo, complete archive/restore and same-binary restart. Preserve every old and new original and receipt. Only then expand duration and recording scope. The full workstation goal and thirty-minute/physical/listening/MIDI/model/plugin/installer/Windows gates remain unfinished.

## Build-profile finding after phase-A measurement

The coordinator's phase-A statement reuse passed 113 recording tests, including nine new private/independent checks. Fresh clone measurements of the same preserved 39.434667-second source now report:

| Build / implementation | Owned view | Semantic pass | Complete prefix | Finalization | Four-phase total |
| --- | ---: | ---: | ---: | ---: | ---: |
| Debug baseline | 3.827115 s | 3.998128 s | 2.967590 s | 16.675032 s | **27.467864 s** |
| Debug, statement reuse | 3.407878 s | 3.949549 s | 2.552965 s | 15.162225 s | **25.072617 s** |
| Release, statement reuse | 0.321796 s | 0.436704 s | 0.257765 s | 1.523753 s | **2.540018 s** |

Read-back sources: [baseline](../../.runtime/rust-seal-fix/seal-cost-baseline-39s.json), [statement reuse](../../.runtime/rust-seal-fix/seal-cost-reused-39s.json), [optimized](../../.runtime/rust-seal-fix/seal-cost-release-39s.json) and its [build/test log](../../.runtime/rust-seal-fix/seal-cost-release-39s.log). All three compared all 7,556 original chunks, retained original identity and minted no native authority. The optimized test artifact is `target/release/deps/omniusong_engine-cb90bfba3ff5aa6a`; its Cargo fingerprint records no enabled crate features and no extra Rust flags. It is still a `cfg(test)` helper, not the native app binary.

Statement reuse alone reduced this sample by about 8.7%, leaving debug well beyond ten seconds. The release helper is about 9.87 times faster than the reused debug helper and finishes this measured workload within the unchanged deadline. These are single-run observations, not a distribution or hard timing guarantee. Both Rust code and bundled native dependencies changed optimization profile. Current retained `libsqlite3-sys` build outputs confirm separate debug/release static libraries and no CFLAGS override, but do not retain their exact C compiler command lines; do not attribute the whole difference specifically to SQLite `-O0` without another measurement.

The source packaging discrepancy is direct: `scripts/build-native-mac.sh` invokes `cargo build --locked -p omniusong-app` and copies `target/debug/omniusong-app`. The workspace already declares a release profile with thin LTO and one codegen unit. **Recommendation: make the native Mac development bundle optimized by default, with an explicit validated `--debug` override, before attempting transaction or watchdog redesign.** “Release profile” describes compilation, not product readiness, notarization or a released installer. Retain the stable development bundle identifier and truthful preview status.

The packaging interface should select exactly one build profile, reject unknown/conflicting options before building or replacing a bundle, and copy only the successful invocation's executable. Make the target directory explicit or use Cargo's emitted artifact path; do not silently copy a stale conventional path when a target-directory override is active. The selected profile, build command/features, source revision plus changed-source fingerprints, toolchain/target and final executable SHA-256 belong in the package receipt. Print the selected profile during packaging. Preserve the existing previous-bundle retention and signing checks. An explicit debug bundle remains useful for diagnostics and carries its own identity and performance limitations.

Profile qualification implications:

- Repository search found no local production `debug_assert!`/`debug_assert_eq!`/`debug_assert_ne!` checks and no debug-conditional recording validation. The only local `debug_assertions` conditional is the app's Windows subsystem attribute. Explicit `require` checks, hashes, metadata/semantic validation, atomics, private permits and watchdog constants remain compiled in release. This does not establish that every dependency has identical debug/release diagnostics.
- The current manifest does not explicitly configure release overflow checks, debug assertions or panic strategy. Cargo's ordinary optimized profile changes implicit arithmetic/debug-assert behavior. For the closest fail-closed arithmetic parity, the coordinator should consider explicit release `overflow-checks = true`, then repeat the optimized measurement and affected release tests with that exact profile before packaging. If retaining ordinary release defaults, record that choice and run overflow/malformed-input regressions in release. Do not add `panic = "abort"` as an incidental speed change; cleanup/unwind behavior requires its own review.
- Keep optimized fault/persistence/lifecycle tests and the default-feature packaged app as distinct artifacts. `--all-features` includes test hooks; a passing optimized test helper is not evidence that an app with a different feature set or prior source is the same binary. Release-profile tests must preserve original-row identities, pre/post-commit outcomes, deadline/cancellation races and response fences.
- `main` passes its executable path from `argv[0]` to `EngineClient`; the supervisor launches that path with `--audio-worker`. The GUI and worker should therefore be verified from the same newly packaged executable, with old processes exited before package replacement/launch. `ENGINE_BUILD_ID` includes package version and live-protocol version but no optimization profile or source digest. It establishes protocol compatibility, not exact debug/release identity; retain the actual executable hash. A profile-only packaging change does not itself require a wire or journal schema change.
- Run repeated optimized profiles, then the fresh optimized bundle's actual comparable output-only Finish/normal-seal/attachment/same-generation playback/history/archive/restart workflow. Keep original sources and diagnostics intact. Passing this bounded workflow can qualify the immediate packaging/performance correction; it does not establish the thirty-minute capacity, recovery or physical listening gates.

No transaction/pass reduction or watchdog change is authorized by this measurement. Keep those sections as the later scalable strategy. Even a naive linear extrapolation from 2.54 seconds at 39.43 seconds would exceed ten seconds for much longer material; extrapolation is not a measured bound. Obtain a defined long-session envelope, real larger fixtures and the actual thirty-minute evidence before claiming that optimized packaging alone resolves sustained recording.
