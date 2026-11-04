# Held projection binding: independent evidence audit

October 22, 2026. **Passed: true.** No missing reference, fingerprint mismatch, test-count mismatch or unresolved scope blocker was found in this bounded audit.

The audited [final packet](../verification/rust-native-projection-held-binding-tests.json) is 59,777 bytes with SHA-256 `ccb5a0dfed63703d64dfa5c5677741a0b00cb8d047af866b84dad9f965eb6643`. Its baseline is `70195019d6b80336dde26bb7a7f3f0c64ee8af0c`. Acceptance covers the held-source/binding libraries and pure complete candidate prerequisite. It does not accept atomic store publication or musician-facing projection integration.

I read and hashed existing source/artifacts and parsed raw logs and stored preservation JSON. I ran no Cargo, formatter, build, native application, UI, device, input/output or SQLite inspection. The regular catalog was not opened or rehashed directly; its preservation evidence comes from the exact stored before/after snapshots. Only this new audit document was written. The existing source review and tested source files remain unchanged by this audit.

## Fingerprints and stable inputs

The packet and its [final gate record](../../.runtime/rust-projection-held/gate-first/gate-run.json) contain **769 fingerprint appearances identifying 266 unique paths**. Every referenced file exists and matches its recorded SHA-256 and byte length, with no conflicting fingerprints for a repeated path. This includes all eight final raw command logs, the diagnostic logs, contract, frozen source review, preceding ownership packet, preservation records and installed binary. The preceding packet was checked as an exact historical artifact, not treated as a fresh qualification of its older source set.

All **247 current source inputs** match their fingerprints. The packet's source array exactly equals the gate's before array, and the before and after arrays are identical. The packet's eight command records exactly equal the gate's command records. The gate record itself is 99,053 bytes with SHA-256 `12e9604d24a1557199b2e4249226d3a069f0d1cd89e8756e25703601d1c0fdac`.

The frozen [independent source review](RUST-PROJECTION-HELD-BINDING-REVIEW.md) remains 14,991 bytes with SHA-256 `aac8a3ed17dec3b98cdf2d9354ff2cc3a57315de176364042f5126b2ab02fd23`. Its bounded source decision and explicit distinction between inspected source, observed coordinator diagnostics and unexecuted reviewer tests remain accurate. The [binding contract](../contracts/NATIVE-PERFORMANCE-PROJECTION-BINDING.md) matches SHA-256 `702a90c5d7332a63329db100af70185cc79f38e8697615e3541e8e9d00f466af`.

## Actual final command results

I independently summed each raw `test result` line rather than trusting the packet totals. All eight recorded commands exited zero. Formatting, all-target/all-feature warnings-denied Clippy and the default-feature all-target workspace build completed successfully in their recorded runs.

| Recorded test command | Passed | Failed | Ignored | Result groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,256 | 0 | 9 | 72 |
| Optimized core projection cases | 16 | 0 | 0 | 1 |
| Optimized complete performance library | 37 | 0 | 0 | 1 |
| Optimized actual-source projection integration | 19 | 0 | 0 | 1 |
| Optimized staged store-schema regressions | 12 | 0 | 0 | 1 |

The optimized total is **84**, derived from actual command arguments containing `--release`: 16 + 37 + 19 + 12. These overlap the workspace cases and are profile-specific checks, not 84 additional unique tests. The workspace raw log is 147,107 bytes with SHA-256 `17ad936a762a9d204529850438658b0913e01425cdfcb232c6f0e296dbec861b`.

I extracted all **22 new test names** from the changed sources and matched each to an `ok` result in both the workspace log and its corresponding optimized log: six `complete_candidate_*` cases in core, seven shared binding cases, and nine `binding_review::*` actual-source cases. None is missing, filtered out or merely inferred from a suite count.

## Earlier diagnostics remain failures or no verification

The first combined filtered diagnostic selected **zero tests across 18 result groups** and supplies no verification. The initial engine diagnostic recorded **10 passed and 9 failed**: all nine new cases faulted during synthetic setup before binding assertions because first note/voice counters were not contiguous from 1. The corrected fixture uses source-1 counters 1 then 2, source-2/source-3 counters 1, and matching Note Off references; the production renderer guard was not relaxed.

The retained corrected engine diagnostic records **19 passed, zero failed**; the projection-library diagnostic records **24 passed, zero failed**. All five diagnostic log fingerprints match. The packet and source review describe the zero-selection run, failed setup and correction honestly; none substitutes for the final stable-source gate.

## Preservation evidence

The [before snapshot](../../.runtime/rust-seal-native/projection-held-before-tests.json) is 51,310 bytes with SHA-256 `529c120693e3fac0f984c5dd6679b18c6d0b3015924732f35ccf10e9e49d9f72`. The [after snapshot](../../.runtime/rust-seal-native/projection-held-after-tests.json) is 51,310 bytes with SHA-256 `0122b4920a3a43817fa123426d122054a6fdd697a91274e89fb6057e08112475`.

Their timestamps differ. Their complete `tables`, `projectIds`, `files`, `catalogFile`, `integrityPassed` and `binary` fields are equal. I independently counted **13 tables, 403 retained row hashes, 22 project IDs and 80 protected file records**, and checked that every table's recorded row count equals its row-hash array length. The integrity result is the coordinator's recorded result; this audit did not rerun SQLite integrity checks.

Both snapshots identify the unchanged 8,613,888-byte catalog with SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. The recorded schema-6 context remains consistent with the unchanged store schema constant; no fresh schema query was performed in this audit. The installed `OmniuSongNative` file was independently rehashed: **23,062,544 bytes**, SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`, matching both snapshots and the packet. No new installed package is claimed by the gate.

## Documentation and closed admission boundaries

The current README, STATUS, IMPLEMENTATION-PLAN and RELEASE-ACCEPTANCE claims agree with the observed counts and distinguish completed libraries/pure construction from remaining store, archive, durable session and native integration. The clarified plan leaves grouping distinct registered originals to future store work. Every release row remains incomplete; no new package, AI inference or calibrated timing is claimed.

Store, archive, session, recording and NativeReader sources are unchanged by this stage. I read the still-closed guard sites: `projection_boundary::reject_unverified_projection`, active load/reference/history/operation/create/take checks, OSAR `Bundle::references`, copy `Proof::document`, and NativeReader's delegation to the guarded loader. The 12 optimized store regressions provide executed evidence for that existing boundary. Header-only archive inspection still does not authenticate hidden history or copy proofs.

## Qualification limits and required next work

This accepts a macOS software/source prerequisite using device-free recovered v1/v2 originals and pure candidates. It does not accept normal native sealing, a new package, input/loopback, playback/listening, physical timing, long-duration capacity, worst-case memory/latency, Windows, MIDI 2/MPE transport/expression or inference.

Required next work remains registered-source authentication across distinct takes; atomic typed document/history/receipt publication and generic registry immutability; authenticated Undo/Redo; historical commitment separated from current usability; durable exact-request admission/uncertainty/restart; complete source-aware archive closure and NativeReader activation; then actual native preview/confirm/edit/history/archive/restart/audition. A candidate, digest or internally consistent receipt grants none of those authorities. Full milestone 2 and the complete Rust workstation goal remain open.
