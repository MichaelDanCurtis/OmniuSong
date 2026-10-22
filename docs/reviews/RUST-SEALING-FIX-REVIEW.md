# Independent longer-seal fix acceptance criteria

October 21, 2026. Initial read-only source review against checkout `e95d5e9` on `codex/native-recording-foundation`. This document establishes review criteria; it does not accept an implementation, attribute the retained native failure, or report a native qualification run. No Cargo, formatter, native input/device/UI, packaging, catalog, or original-journal operation was performed by this reviewer.

## Evidence and retained boundary

The prior [seal-cost report](RUST-RECORDING-SEAL-COST.md) measured 6.573177 and 12.795799 seconds for equivalent trusted storage work, against the original ten-second sealing deadline. Those are two debug-build observations. They make deadline expiry plausible but do not establish the native cause. The [retirement review](RUST-PERFORMANCE-RETIREMENT-REVIEW.md) separates publication of a known cause from delivery of that cause to the parent.

The current normal-seal path in `performance_writer.rs` performs an owned-view generic scan and original-row/physical-file authentication, a complete semantic scan, a complete-prefix snapshot, then checked transactional sealing. `journal.rs` revalidates the admitted complete receipt and original rows before finalization, checks every selected original chunk, and checks originals/complete state/manifest again after insertion and before commit. Each validation boundary presently has a distinct purpose. Reducing SQL preparation cost does not justify removing any of them.

The original failed stage and its rollback sidecar remain evidence. No profiling or test acceptance may delete a sidecar, open that stage for recovery/write, mutate the original, invent Finish or callback receipts, or treat a copied journal as native lifecycle proof. Synthetic journals and isolated copies must have fresh explicit paths.

## Prepared SQL and row-hash optimization

Statement reuse is a reasonable first optimization. Reusing a completed digest is a different change and is not accepted by these criteria: the owner lock alone does not establish that a later transaction sees the same original rows, and schema triggers may participate in a write transaction. The checked pre- and post-write all-row authentications must remain actual reads at their existing transaction boundaries.

The optimized hasher must preserve the existing `OmniuSong.capture-original-rows.v1` byte encoding exactly, including the following:

- Table order, column order/names, rowid order, row counts, and type markers.
- Signed integer bytes, real-number bit representation, NULL handling, text/blob lengths, and exact bytes from bounded pieces.
- Schema rows, descriptor fields other than the intentionally excluded take state, stream prefixes, every chunk including damaged/unselected tails, and SQLite sequence state.
- Existing rejection of unknown tables, oversized or reserved metadata, invalid lengths, truncated reads, and SQL errors.
- The same cumulative budget charges, cancellation checkpoints, sticky outer typed refusal, bounded buffers, and no partial-success promotion for bounded inspection.

Any cache must contain bounded prepared statements or metadata whose lifetime and invalidation are justified by the held connection/snapshot. It must not contain whole-take payloads or silently become a cross-transaction content cache. Parameters must be rebound/reset for every row and piece. Identifiers remain bounded and quoted; values remain SQL parameters. A cache miss/error cannot select a shorter authenticated prefix.

Required evidence is digest equivalence against an independent pre-optimization oracle or fixed golden fixtures, using multiple rows and columns, NULL/integer/real/text/blob values, empty and multi-piece values, unusual quoted column names, and damaged/unselected tails. Include a fixture that changes one original field between authentications and a transactional mutation after manifest insertion: the checked seal must reject or roll back. Unknown schema and cancelled/limited bounded readers must retain their prior refusals. Existing lost-ack replay and conflict tests must remain valid.

Measure more than one take size and repeat timings with the same build/profile, inputs and phase boundaries. Report elapsed phase values and original-row identities; do not make performance correctness depend on a tight wall-clock unit assertion. Preserve byte-for-byte sources and directory listings in any clone-based profile.

## Scalable sealing watchdog

A replacement watchdog must remain bounded while allowing larger valid work. A larger fixed timeout alone does not satisfy the requirement. The implementation should make its maximum abandonment time explicit and measurable, using admitted work or bounded per-phase work/progress with an overall bound. The chosen policy must cover blocking SQL, I/O and commit/destruction intervals where no progress callback can run.

Progress may extend a lease only when it represents strictly advancing validated work under the current seal attempt and phase. Repeated polling, retries of the same row, an unchanged counter, generic status publication, or activity on another phase/attempt must not renew it. Receipt phase transitions and counters are observations; they cannot authorize native sealing or substitute for row authentication. Every per-unit callback must be off the audio callback path, bounded, and free of native lifecycle work.

The existing lifecycle ordering must survive:

- A stale START/FINISH/STOP/DRAIN watch or progress event cannot replace a later sealing watch, extend its deadline, or clear a newer watch.
- A watchdog observation claims the exact attempt/phase/deadline it expires. A concurrent valid handoff or completion must not be retired under an obsolete observation.
- Stop and normal authorization still have exactly one cancellation-CAS winner. Stop before authorization leaves the original unsealed; Stop after authorization cannot assert transaction rollback.
- A stalled or failed seal retires admission, preserves writer/original evidence and reports the phase/cause through the existing immutable cause and response fence. Expiry cannot fabricate a successful seal, attachment, callback retirement or a new native permit.
- A previously validated manifest remains a known `Sealed` observation even if final cleanup times out. Before a validated manifest is published, an authorized ambiguous transaction remains `SealUncertain`; an error cannot claim it is definitely uncommitted.
- Retry retains the exact writer, permit, operation identifier and complete receipt. Retry must not renew a stalled attempt indefinitely, mint fresh authority, rerender, or fall back to recovered/shorter-prefix completion.

Use a deterministic injected clock/phase hook for deadline tests. Cover progress just before expiry; no progress; duplicate/regressing progress; stale phase and stale attempt events; expiry versus completion/handoff races; Stop before and after authorization; retry after a committed lost acknowledgement; failure before/after manifest insertion and commit; and a validated manifest waiting for writer destruction. Check bounded exit and exact immutable diagnostic, not merely a displayed generic error. Any changed budget values need measured justification and explicit upper bounds/overflow handling.

## Acceptance remains staged

Source-level acceptance requires independent final diff review and the coordinator's complete source checks after the freeze. Storage timing and synthetic protocol tests do not qualify a native recording. Native qualification requires a fresh comparable-duration software-output recording that observes the actual cause if it fails, then Finish, explicit attachment, same-generation Play/Status/Stop, history, complete archive and restart/restore checks. Thirty-minute reliability, physical input, audible quality, AI inference, plugin hosting, installer and Windows acceptance remain separate open gates.

The frozen first implementation slice is reviewed below. The watchdog criteria remain requirements for a separate future change; this slice changes no deadline or liveness protocol.

## Independent test contribution

The coordinator subsequently authorized only `crates/omniusong-recording/tests/original_rows_hash_independent.rs` in addition to this document. Its three synthetic tests implement a whole-value `SELECT rowid,*` / `ValueRef` encoding oracle, independent of production's per-column type/length/piece reads. They compare public full-prefix receipts and the owned inspection hash before and after a checked seal; reject an old receipt after an opaque original changes; and keep damaged and unselected tail rows inside authentication. The fixtures exercise all five SQLite storage classes, empty values, text containing NUL and Unicode, quoted/reserved extra column names, text/blob values beyond two 64 KiB pieces, and eight PCM chunks each larger than 64 KiB. Each synthetic journal asserts a file size below 10 MiB.

The tests were source-reviewed but not executed or formatted by this reviewer. The coordinator owns execution, formatting and the final integrated gate. No new dependency or test hook is requested by this contribution.

## Frozen SQL-preparation slice disposition

**No blocking safety finding in the reviewed first slice.** Its only production change is statement reuse in `inspection.rs`; acceptance here is limited to that implementation's source semantics. It does not claim a sufficient speedup, successful longer native Finish, or an accepted watchdog redesign.

`OriginalFieldQueries` owns exactly three prepared statements for each included column: type/length, scalar value, and bounded byte piece. The existing 128-included-column limit bounds these bundles to 384 field statements for the current table, plus the fixed row iterator. The vector lives inside one table's traversal and is destroyed before moving to the next table or returning from the hash call. It retains no row digest, payload, cross-transaction authentication fact, connection-global cache entry or native authority.

The prepared SQL has the same expressions and positional parameters as before. Column quoting still escapes embedded quotes, while table names remain fixed. Every field still reads its type and byte length first; integers/reals retain their existing interpretation; text/blob data is still read as at most 64 KiB of cast-to-BLOB bytes, with exact length checks and the same hash encoding. Table/column/row iteration order, type markers, lengths, rowid values and counts are unchanged. The implementation does not alter any caller, full authentication pass, transaction, complete-prefix/manifest check, stored format or sealing timeout.

The pinned local `rusqlite` 0.40.2 source confirms that `Statement::query_row` binds the supplied parameters for each call and owns a temporary `Rows`; its destructor resets the statement. Every query call here supplies its exact positional parameter count, including rowid, offset and count for each byte piece. Type changes across rows and short values after multi-piece values therefore cannot reuse an earlier result or stale offset.

Existing logical row/work charges are unchanged and remain cumulative. Three additional checkpoints surround preparation, so cancellation can stop earlier while preparing the bounded statement set. They neither reset usage nor convert refusal to a usable prefix. The actual SQL VM hook remains installed on the same connection and governs executions of reused statements.

The coordinator's completed `.runtime/rust-seal-fix/hash-recording-tests.log` was read after the freeze. It records all six new private compatibility/budget tests and all three independent whole-value-oracle tests passing. The private tests compare the frozen pre-optimization implementation across mixed types, invalid UTF-8 stored as text, byte boundaries, empty tables, 128 included columns, exact logical charges, sticky cancellation and a live VM-hook refusal after accepted rows. The independent tests additionally exercise public complete receipts and checked sealing, rejection after a changed opaque original, and retained damaged/unselected tails through a different query/encoding path. Existing complete-prefix trigger rollback, conflict and lost-ack replay groups also pass in that log. These are the coordinator's executable results; this reviewer ran no Cargo or formatter.

| Frozen source | SHA-256 |
| --- | --- |
| `crates/omniusong-recording/src/inspection.rs` | `a32d73bb2e4be9ee92a37086f57116f66794ab921aa4ddefd947765b5b470f35` |
| `crates/omniusong-recording/src/inspection/hash_tests.rs` | `15e913d757cb23fa87d386e3fbcef71c8064791843d98c09d51aab2bd8c47373` |
| `crates/omniusong-recording/tests/original_rows_hash_independent.rs` | `824df5fdc54662e2f2a1372bd874b1d99b3cb99daf5b1e7c76fb773730b2e64b` |

The coordinator's newer diagnostic run reports an exact sealing deadline retirement after 1,892,864 frames and 3,697 callbacks (39.4346667 seconds). This confirms the cause of that new run; it does not retroactively identify the earlier generic failure. Its original remains retained and unattached. Repeated isolated measurements and fresh native end-to-end qualification of the optimized build are still required before a recording-reliability claim. The complete integrated workspace gate also remains the coordinator's responsibility.

## Optimized development-bundle profile decision

The statement-reuse debug measurement alone is insufficient: on the same 39.4346667-second source its four-phase total is 25.072617459 seconds, compared with the preserved debug baseline's 27.467864167 seconds. Both exceed the unchanged ten-second seal deadline. These are individual observations, not distributions.

The coordinator then ran the identical helper with Cargo's existing `--release` profile. The reviewed log records `Finished release profile [optimized]` and the executable `target/release/deps/omniusong_engine-cb90bfba3ff5aa6a`. The corresponding `.runtime/rust-seal-fix/seal-cost-release-39s.json` records:

| Equivalent storage phase | Release seconds |
| --- | ---: |
| Owned inspection | 0.321795959 |
| Held-view semantic scan | 0.436704041 |
| Complete-prefix authentication | 0.257765000 |
| Checked complete-prefix seal | 1.523752959 |
| Four-phase total | **2.540017959** |

This receipt matches the debug receipt's 7,556 chunks, 1,892,864 frames, 3,697 callbacks, descriptor/checkpoint/chunk-index/original-row/complete-prefix identities, source size/hash and source directory names. It reports every source chunk compared, the original unchanged, and no device, catalog or native authority. The final clone/manifest hashes naturally differ because each fresh seal has its own manifest identity. Source open, preflight, copying and final comparisons remain outside the timed four-phase total and are separately reported.

At review, the named release helper's SHA-256 is `5466ff09dcd0f415991dcbcdbab70826eafbe33a45cc8f96876bcc77477ae590`; its unchanged helper source is `84c590297219020d18a9ecb322bac739093312b1ccdcd0701c5dc7be6b36b451`. The preserved `e95d5e9` Cargo manifest without the subsequent release-overflow addition hashes to `4fd20d1ab60aad00fcc869fb25e719f6e9101a1c9aa128181083e5898f5e644d`. The receipt does not embed a build-profile field, so preserve its log and binary identity alongside it. The 2.54-second observation predates the explicit release-overflow setting and must not be presented as its final-profile measurement.

**The measurement supports release-default packaging as the next narrow change**, retaining an explicit debug override. `scripts/build-native-mac.sh` currently always runs a dev build and copies `target/debug/omniusong-app`; packaging the optimized build avoids imposing that unoptimized cost on the native preview. `Cargo.toml` already defines release thin LTO and one codegen unit, so this decision requires no optimization-profile, timeout, authentication or authority change. The result is still an ad-hoc signed development bundle with its existing preview identity, not a qualified production release.

The reviewer independently searched application-owned Rust sources for debug assertion/profile branches. No `debug_assert!` or debug-assertion-gated validation was found in engine, recording or store. The app's only `debug_assertions` configuration controls the Windows GUI subsystem attribute. Mandatory Result-returning validation and ordinary assertions remain in the release helper. Existing panic settings are not changed. Release code generation and arithmetic behavior still require release-profile regression execution; a debug source gate alone does not establish that equivalence.

The package script must select one whitelisted profile and use it consistently for Cargo arguments and the copied artifact. An explicit debug override must select dev/debug in both places; unknown/contradictory arguments must fail before build or bundle mutation. Preserve previous bundles, existing ad-hoc identity and unchanged application data. Record the selected profile, exact build command, source fingerprints and copied/signed binary identity. Check inherited target-directory configuration so the script cannot package a stale default-path executable after building elsewhere.

Review of the eventual script diff and its profile-selection checks remains pending. Repeat release measurements to observe variation, run relevant release-profile storage/engine regressions, and qualify the exact new package through fresh recording, Finish, explicit attachment, same-generation playback/status/Stop, history, archive and restart/restore. A single 2.54-second helper result neither guarantees a native deadline nor establishes a scalable thirty-minute sealing envelope. No failed original may be reused as a new native success or attached because a clone sealed.

## First frozen package-script review

The next frozen diff adds `overflow-checks = true` to the existing release profile and makes the Mac builder select release by default or debug explicitly. Preserving overflow checks is appropriate arithmetic safety parity; mandatory validation and seal authority remain in production code. The argument parser accepts one whitelisted selection/help and rejects unknown or multiple arguments before Cargo. A single selection drives both its argument array and copied profile path. The Info.plist profile entry is written before lint/signing. Existing staging, ad-hoc identity and previous-bundle preservation remain intact.

The initial script revision explicitly fixes the target directory but still copies `target/<profile>/omniusong-app`. The reviewer reported a provenance gap to the coordinator: inherited `CARGO_BUILD_TARGET` or a Cargo `build.target` setting can instead emit `target/<triple>/<profile>/omniusong-app`, leaving the script to package an old default-path binary. This needs an explicit host target and matching artifact path, or validated selection of Cargo's actual executable artifact. The package path must never be inferred from the profile alone while the target can vary. Separately, the final build receipt must establish the effective overflow-check setting because Cargo configuration/environment can override the manifest. Script acceptance is pending the narrow guard correction and final-profile evidence; the in-progress release workspace test log is not yet a completed gate.

## Revised frozen package-script disposition

**The artifact-provenance finding is resolved in the revised source. No further source blocker was found in this packaging slice.** The revised script uses Cargo's JSON artifact stream and macOS `plutil` to select the actual executable, rather than guessing from a target directory. It binds the app artifact to its exact source path, binary kind and target name; requires one executable; and retains the selected and application-owned artifact records outside the installed UI. All seven known application-owned target names must be present. Duplicate owned target names, missing/invalid records, wrong optimization level, disabled Cargo-reported overflow checks and `test-hooks` features fail before bundle replacement.

The script accepts only non-test artifacts with release optimization level 3 or debug level 0 according to the selected profile. It rejects a nonhost `CARGO_BUILD_TARGET` before Cargo, and accepts the app's reported path only in this workspace's selected profile directory or exact macOS Rust host-triple/profile directory. A configured nonhost target therefore cannot cause a stale default-path executable to be copied. `lipo` additionally verifies the running host architecture. This is a path/architecture guard, not a new claim that architecture alone authenticates a Mach-O platform.

Unknown, empty and multiple arguments reject before Cargo; help exits before build/platform work. The target directory remains explicit. Selected profile/optimization/overflow metadata is inserted before plist lint and ad-hoc signing; existing bundle replacement still occurs only after successful staging/signing. None of this changes a validation, on-disk recording format, native permit, timeout or protocol identifier. The same bundled executable remains the private worker. Cargo artifact profile records establish Cargo's reported target settings; exact invocation, configuration/flags, source, binary and signature evidence remain necessary in the coordinator's package receipt.

The revised script SHA-256 is `f2bc1e59441832207c64064dc6b1b1e47e270a10f596ce4f5060d3581b24c10b`. The final manifest with explicit release overflow checks hashes to `016c8559c0dc0b5a08610c9d8ac732ffa01f39d9d8c24309db8996149ea0b41a`. The coordinator owns actual parser/argument checks and package execution; this reviewer only read source and evidence.

The completed coordinator log `.runtime/rust-seal-fix/release-workspace-tests.log` records **820 passed, zero failed, five ignored, 62 result groups** under the final release-overflow profile. The reviewed log includes the new hash compatibility/oracle groups and the existing seal/retirement checks. This is all-features optimized source evidence, distinct from the default-features package.

The coordinator's subsequent final-profile helper log and `.runtime/rust-seal-fix/seal-cost-release-overflow-39s.json` record a **2.602231833-second** four-phase total: owned inspection 0.333956500, semantic scan 0.452227667, complete-prefix authentication 0.262104666, checked sealing 1.553943000 seconds. It again matches all 7,556 chunks, the descriptor/checkpoint/chunk-index/original-row/complete-prefix identities, source bytes/hash and directory names, with every chunk compared and no device/catalog/native authority. This replaces the earlier 2.54-second observation when describing the final configured profile. It is still one final-profile measurement and no native acceptance.

That log names `target/release/deps/omniusong_engine-3401eefa33094f51`; the reviewed executable's SHA-256 is `fbcdc4adaaf7e61db4179d5ad55696a029a0c6dc5f9e1556c4db4fe1d5b8d908`. It is a manual test helper, not the packaged default-features application.

The exact default-features optimized package and fresh native recording/attachment/playback/history/archive/restart workflow remain the next acceptance boundary. Repeated timing/capacity work and a separately reviewed scalable sealing policy remain open for the full long-recording goal.
