# Atomic projection publication: independent evidence audit

Date: 2026-10-22. **Decision: bounded acceptance; passed: true.** No missing or mismatched evidence, test-count discrepancy, or unresolved serious source finding was identified for this Rust library/store slice. This audit does not qualify native projection controls, complete archive closure, the parallel test harness, or the full workstation goal.

The accepted [verification packet](../verification/rust-native-projection-publication-tests.json) is **67,084 bytes**, SHA-256 `092d7c13f1fbaecbfb4ca6dc366d4bd88d3c19402ec28af128e5a71a56e471a0`. This is the final wording-corrected packet: it distinguishes NativeReader refusal on a nonempty current document from OSAR refusal on both current ownership and ownership hidden in history by Undo. Hidden-history NativeReader behavior is explicitly unqualified. The earlier `d9e180c9…` packet digest is superseded.

I read and hashed actual files and raw logs, compared recorded snapshots, and inspected the relevant test assertions and remaining source guards. I ran no Cargo command, application, device operation or SQLite reader, and did not open the default studio catalog or protected originals. The only file written by this audit is this report. Test execution belongs to the coordinator's recorded commands, not to this reviewer.

## Evidence identity and executed results

I checked **813 fingerprint appearances representing 294 unique path/hash/length records**, with **zero missing files or mismatches**. Coverage includes the packet's direct references, all final gate source snapshots and raw logs, historical gates' command logs, and the fixed-source standalone probe's source/executable/log references. Historical failed gates retain their own earlier source snapshots; those artifact hashes were checked without incorrectly demanding that their internal historical source hashes match today's corrected files.

The [final gate record](../../.runtime/rust-projection-publication/gate-final/gate-run.json) is **102,773 bytes**, SHA-256 `bfdb9f6ec8561b7a6300093fdfd8fbbece8d2901e2ff19277af1389bc052f026`. Its **254 before/after input records match exactly**, and each current qualified input matches its recorded byte length and SHA-256. The packet's source and command arrays exactly match the gate. All nine recorded commands exit successfully, including formatting, warnings-denied all-target/all-feature Clippy and the default-feature all-target workspace build.

Recounting actual `test result` lines gives:

| Recorded test command | Passed | Failed | Ignored | Result groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,283 | 0 | 9 | 72 |
| Optimized core projection | 16 | 0 | 0 | 1 |
| Optimized performance library | 37 | 0 | 0 | 1 |
| Optimized engine source/binding integration | 34 | 0 | 0 | 1 |
| Optimized store library | 150 | 0 | 1 | 1 |
| Optimized store schema regression | 12 | 0 | 0 | 1 |

The five optimized commands therefore execute **249 passing tests**. I extracted all **27 new case names** from the four recorded source files: seven publication/receipt DTO cases, three error-domain cases, two metadata-prepayment cases and fifteen actual synthetic publication integration cases. Every name appears exactly once as successful in the workspace log and once in its corresponding optimized log. These are named executed assertions, not inferred coverage from a filter or total.

Every final test command explicitly records `-- --test-threads=1`. This serializes the harness, not tests' own concurrency. Source and successful workspace rows confirm that the two-writer source-action races and concurrent retirement/publication still spawn synchronized threads; the append-group interruption and SQLite-owner exclusion tests still launch their explicit child processes. The ignored child-helper rows must not be counted as standalone passes. This evidence does **not** qualify simultaneous execution of the whole harness.

The packet binds the [publication contract](../contracts/NATIVE-PERFORMANCE-PROJECTION-PUBLICATION.md), **10,472 bytes**, SHA-256 `3e27a4daa69e1cee7174a3aed44720b51eeaf8d505a1d9654703d87711e687de`, and the frozen [independent source review](RUST-PROJECTION-PUBLICATION-REVIEW.md), **17,019 bytes**, SHA-256 `d47dd733ac8e3011c24e1a0cc0db4d4c46f06f903e3def7838f451c8423300d9`. Their bounded acceptance and limitations agree with the final evidence.

## Preservation

The actual [before snapshot](../../.runtime/rust-seal-native/projection-publication-before-tests.json), SHA-256 `6b884bc39c4263fb356621ade97a069fad6493edd023d0b0d5658688301e8da0`, and [after snapshot](../../.runtime/rust-seal-native/projection-publication-after-tests.json), SHA-256 `35b45f8dda66162061799ced492d2ebdc61b4416dc0a6ea0e318512883c0335b`, are each **51,310 bytes**. Excluding their capture timestamps, they agree exactly: **13 tables, all column lists and 403 row hashes, 22 project IDs, 80 protected file path/hash/length records, the physical catalog fingerprint, and the installed binary fingerprint**. Both record successful integrity checks.

The unchanged catalog is **8,613,888 bytes**, SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. This carries forward the accepted schema-6 baseline; the current source constant also remains 6. The snapshot format does not independently record `user_version`, and this audit did not issue a new PRAGMA or reopen the catalog. Protected originals were compared through the snapshots, not read again.

I independently hashed the actual installed `dist/OmniuSong Native.app/Contents/MacOS/OmniuSongNative`: **23,062,544 bytes**, SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`, matching both snapshots and the packet. There is no new package, installed publication implementation, or native walkthrough claim.

## Retained failures and bounded diagnosis

The historical evidence remains visible and must not be relabeled as successful:

- The first gate stops at warnings-denied Clippy on the unused test import `Value`.
- Two separate parallel store runs each report **149 passed / 1 failed / 1 ignored**. The first receives `Owned` in the full-root archive fixture; the second receives `Recording(Owned)` during inert-restore fixture setup, before its namespace assertions.
- The intermediate serial gate stops with the schema-regression group at **9 passed / 3 failed**. The old helper requires a projection-message substring, while the new active authentication path correctly refuses the audio-only fixture with `No owned performance namespace`.
- The corrected helper accepts that exact message only as `StoreError::Recording(RecordingError::Invalid(...))` on the active original-proof paths. Other refusal reasons remain separately exact, staged guard assertions remain, and SQL/file preservation oracles remain. The narrow navigation correction reports an absent selected history edge as `Nothing to undo/redo` before payload acquisition; it grants no source authority. The focused corrected run executes **12/0**, followed by the complete final gate above.

The accepted diagnostic is only [lock-spawn-final-results.json](../../.runtime/rust-projection-publication/lock-spawn-final-results.json), **3,352 bytes**, SHA-256 `91e571d816d93a25c5f8c1ecd398763ebff63fd96c609413f735338a9463eef4`. Its fixed source is SHA-256 `4c04be56ff5cc177c9ff532b1f60cf28e07e2a3297c126b207d63b48df4a4431` before and after compilation/execution; the bound executable is SHA-256 `b5d504bdcdeede5b387ea0f72a2e66360e6ca481439138033431cff0cc88e04a`.

Reading that source and both raw output logs confirms **5,000 baseline rounds / zero post-drop refusals**, then **20,000 rounds / 2,228 post-drop `WouldBlock` refusals with 8,678 successful plain spawns / zero spawn errors**. The latter run uses ordinary `Command::output`, with `force_fork=false`. This reproduces a standalone standard-library lock/process-spawn interaction without opening a journal, catalog, project or device. It neither identifies the exact holder or syscall behind the earlier suite failures nor measures recovery timing. The earlier instrumented result remains explicitly **unqualified** because concurrent source edits broke source-to-executable provenance; its counts and timing are not substituted into this acceptance.

## Scope that remains open

The qualified behavior is source-aware atomic store publication, current load, ordinary editing/deletion, immutable original ownership, one-operation history, and compact exact historical commitment replay. Source-absent replay is a recorded commitment, not authenticated current musical usability or permission to reinstall an old snapshot. The unchanged resource ceilings and shared cumulative governor do not establish worst-case combined capacity.

**Reusable main-catalog SQL cancellation is between statements only.** No scoped main-connection progress hook is installed, so a single statement or catalog trigger is not bounded by the source-reader VM governor. Rollback-safe scoped catalog VM/cancellation and responsiveness qualification remain prerequisites for native jobs.

The remaining nonempty projection fences are still present in NativeReader/current metadata, list/attachment and OSAR paths. Actual tests cover NativeReader's nonempty current document and OSAR's current plus Undo-hidden history; they do not qualify hidden-history NativeReader, fully reindexed hidden copy-proof attacks, or complete source-semantic archive adoption. Source-aware OSAR current/history/result/copy-proof closure and remapping, schema-3 inert OSRE roundtrip, guarded consumer integration, durable exact-request session admission and exact-binary native review/confirmation/edit/history/restart/audition remain required.

The current top-level delivery statements correctly describe a bounded backend gate. Earlier blanket-refusal wording is historical and must not override this selectively opened store boundary. No result here establishes native input/output or listening, calibrated timing, AI inference, long-performance capacity, MIDI 2/MPE transport, plugin/model integration, Windows, or completion of milestone 2 or the full goal.
