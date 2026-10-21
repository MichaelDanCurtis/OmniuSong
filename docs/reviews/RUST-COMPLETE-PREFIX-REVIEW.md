# Complete-prefix normal finalization — independent review

October 20, 2026. **Source and software persistence review accepted within the scope below; final gate5 and a short actual native proof3 also passed.** The final workspace gate passed 709 cases, including all eight independent cases, with formatting, warning-denied Clippy and default all-target build. Earlier failed native attempts remain retained. This reviewer inspected logs, source bindings and isolated artifacts without running Cargo or opening a device. This recording-library primitive authenticates storage; actual Finish, callback destruction and cancellation authority belong to the native session implementation.

## Reviewed boundary

`Journal::complete_prefix` returns the descriptor hash, normalized original-row hash and full chunk count/index. `seal_complete_prefix` accepts that exact receipt under an Immediate transaction. Source review confirms that a first seal requires Recording state; an exact already-normal seal may replay. Interrupted/selected-recovered originals cannot become normal, even under the same operation spelling. The normal primitive requires only the supported v2 ownership marker, rejects gaps and malformed or additional physical rows, validates all stream prefixes, and cannot choose a shorter recovery prefix.

The normalized digest covers the original descriptor columns, schema, every original chunk column, stream-prefix rows and SQLite sequence metadata with typed values. It is distinct from the chunk index and a physical-file hash. The complete transaction must preserve original rows while adding the normal manifest and changing state. A rehashed replacement can be valid storage yet still differ from the previously admitted original; the old receipt must reject.

`OwnedJournalView` borrows the existing owner/connection and holds a read transaction. It neither reacquires the sidecar lock nor converts into a recovery writer. Ordinal access is restricted to the verified prefix. Dropping the view releases its transaction while the Journal retains exclusive ownership. This permits the engine to inspect its own journal without reopening it through `CaptureReader` and colliding with its owner lock.

## Findings and source dispositions

1. **Final-write mode validation was missing.** Before the correction, finalization checked that recovery tables were empty before inserting the manifest, but afterward checked only the original-row digest. That digest intentionally excludes derived recovery rows and take state. An `AFTER INSERT` trigger could add `recovery_index`/`interruptions` rows, or a state-update trigger could revert state, without changing that digest. The coordinator added a Complete-only postwrite check requiring Sealed state, the same full `complete_snapshot`, and a verified stored manifest equal to the return value before commit. The independent test covers all three injections and requires rollback of every table's changes.
2. **Borrowed inspection could mix cached descriptor identity with current rows.** `OwnedJournalView` originally passed the cached Journal descriptor into its inspection without rereading the stored descriptor. A correctly rehashed replacement could therefore yield an inspection with old descriptor identity and new original-row digest. Final sealing would reject later, but that did not make the inspection itself correct. The coordinator added a current `load_descriptor` equality check inside the held transaction. The independent regression replaces the descriptor and verifies rejection without repair or mutation.

Both corrections are visible in source and their independent regressions passed in the central test gate. No additional blocking storage-design finding was identified in this pass.

## Independent cases

`crates/omniusong-recording/tests/complete_prefix_independent.rs` contains eight cases: seven default and one requiring `test-hooks`. All fixtures are tiny synthetic PCM journals in isolated UUID directories. They establish no native device, engine-record codec or callback-lifetime authority.

- Postwrite recovery metadata/state injections reject and roll back all seven journal tables.
- A valid, fully rehashed PCM replacement invalidates the old or mixed receipt; a fresh explicit storage review remains distinguishable.
- SQL text/blob/integer/real/null changes and `sqlite_sequence` changes alter the original digest while leaving the chunk index unchanged.
- Extra physical rows at negative, zero, gapped and maximum ordinals cannot hide beyond a previously verified prefix; rejection retains the rows.
- A live owner prevents a second reader; a recovered manifest cannot be promoted by reusing its operation spelling; an empty original cannot complete.
- Borrowed inspection preserves the live owner and consistent read snapshot, rejects out-of-range ordinals, blocks a noncooperating writer's commit, and releases only its transaction on Drop.
- A rehashed stored descriptor replacement is rejected by borrowed inspection.
- A separate read-only connection observes precommit Recording/no manifest or postcommit Sealed/one manifest atomically. A deliberately lost after-commit response replays the same manifest after reopening, with all table rows unchanged; a different operation rejects.

The coordinator's nine owner tests additionally cover version/marker shapes, selected-prefix and descriptor mismatch, added original columns, missing/extra/inconsistent stream-prefix rows and commit boundary failures. All nine passed in the same central gate.

## Bound source and central evidence

The independently read `.runtime/rust-performance-worker/gate1-run.json` is 25,432 bytes, SHA-256 `de45103db1d5906dfd6c4d7879b47340938af14d838b7efca65299ba50372519`. Its 129 source records and three log fingerprints were checked. All current source records match except the parent supervisor's subsequent mechanical collapsed-if correction, outside this storage primitive. The workspace log reports **703 passed, 0 failed, 3 ignored helper entrypoints across 55 result groups**; the two complete-prefix test binaries report 9/0 and 8/0. Formatting passed. Gate1 Clippy failed on that one supervisor style diagnostic; final Clippy/default build and the rerun are pending at this review update, so gate1 as a whole is not labeled green.

| Formatted source | Bytes | SHA-256 |
| --- | ---: | --- |
| `recording/src/journal.rs` | 58,485 | `5d94de377fdabc10be7b0c4300fc264743fff158e109a36fb201540d11284f30` |
| `recording/src/inspection.rs` | 19,784 | `20042a7056cf9ce834db87b7e65a26b1dfbefe64990ebfce70e623074896c8d9` |
| `recording/src/types.rs` | 25,104 | `7d6f9b2ba6b4756f97f4c44e7ecf0360e68a2948839b3efca33cc29c10d4342e` |
| `recording/src/lib.rs` | 366 | `0910ddcc7714158074a943a72474e5b18290780d2e91dfd2369685a5d1d68e79` |
| `recording/tests/complete_prefix.rs` | 20,044 | `85f2233f7fd863ee74c2c284159371bb11cd4ab31412a3298af8502dbee42a47` |
| `recording/tests/complete_prefix_independent.rs` | 19,849 | `9f2343e184aa463bba90a7343a5243c48081167fc4ff78064d5e66ae37702297` |

Paths in this table are relative to `crates/omniusong-`. The workspace log is `.runtime/rust-performance-worker/gate1-workspace.log`, 78,308 bytes, SHA-256 `2887474911830a15d8f08e8a436ea3fd546b55b457046223e8c81dda370a8ea8`; its recorded command is `cargo test --offline --workspace --all-features --locked` with exit 0.

## Remaining acceptance boundary

Gate3 (`.runtime/rust-performance-worker/gate3-run.json`, 25,853 bytes, SHA-256 `391b1d897ee598a8c7aebcbbe3712bacf5dbcca2c93f14db40ef4c6c4ad1664d`) passed formatting, all-feature workspace tests, all-target/all-feature Clippy with warnings denied, and the default all-target build. The reviewer rehashed its four logs and parsed the same **703/0/3 across 55 groups**. The storage source/test fingerprints above are unchanged. The test log is 77,875 bytes, SHA-256 `9ded1630a72f42fa713d660f3e3a5319229f23f16c4ca1a9e8d0cedb3ad9de80`. This supersedes the earlier pending warning/build boundary without erasing gate1's style failure.

The first actual output qualifier failed on a parent status-revision rejection before committing any original chunk or manifest. The second retained six chunks after correctly rejecting invalid source-local counters in the synthetic fixture. Neither attempt qualifies successful normal finalization. Both are preserved separately from the successful proof3 described below; see the [native source review](RUST-NATIVE-PERFORMANCE-WORKER-SOURCE-REVIEW.md) for their exact boundaries.

The library's public receipt remains trusted lower-layer data, not an unforgeable native capability. The engine must privately authenticate its v2 Finish/End/checkpoint and full drain, obtain real stream/data/error/tap retirement evidence, and linearize cancellation versus seal authorization before calling this primitive. Reopened unsealed journals cannot reconstruct vanished native authority. UI attachment and final song acceptance remain separate evidence.

## Final gate and bounded native invocation

The reviewer verified all 129 source fingerprints and four log hashes in `.runtime/rust-performance-worker/gate5-run.json` (25,851 bytes, SHA-256 `315227c193883f832cd584ce00878850a4250e6499a7d9c06c261e423b4df35f`). All four commands passed. The workspace log reports **709 passed, 0 failed, 3 intentional ignored helpers across 55 groups**; the 9 owner and 8 independent CompletePrefix cases all pass. Every recording source/test fingerprint in the table above is unchanged.

Actual native proof3 invokes the private session path and this normal primitive after a 1,536-frame, 48 kHz stereo output capture. Independent read-only inspection confirms a normal manifest, 11 authenticated chunks, one terminal checkpoint, exact Finish cut, exact same-intent replay and no recovery metadata. All seven journal tables match across retained stage, owned backup and archive-restored object. The separate urgent Stop remains unsealed; it is not promoted by closure retirement alone. This proves the short concrete integration, not a public caller's ability to assert native quiescence.

The final overall receipt is `docs/verification/rust-native-performance-worker.json` (42,640 bytes, SHA-256 `e4c9ead18bb56f6e86d929b222d4bfc9c384b5bfb08922ab2f8a75e768cf3482`); the actual proof is 82,879 bytes, SHA-256 `c1f3fd3f2870bfba3251f89ce60f86e7908221448be294be426017b2bb17d2ed`. The separate native review records the saved binary identities, PCM/events, full archive/object verification and preservation checks. No recording UI, performance-aware recovery/discovery, long-recording or latency acceptance is transferred from this short integration.
