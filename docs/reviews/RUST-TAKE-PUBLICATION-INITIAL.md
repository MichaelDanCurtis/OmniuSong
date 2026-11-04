# Independent native take publication and snapshot review

October 20, 2026, 18:38 UTC, Darwin arm64. Bounded independent review complete: **152 workspace tests passed**, including **28 new independent regressions**; formatting and all-target/all-feature Clippy with warnings denied pass. Three concrete findings were corrected and their regressions pass. No further major defect is identified within these implemented storage/schema boundaries. This does not accept the full native goal, UI workflow, real recording, audio, Windows or permissions. This lane owns independent tests and review, not implementations.

## Boundary under review

The preceding milestone accepted musical schema 1: opaque JSON remained exact and legacy asset discovery followed explicit schema-owned paths. This stage adds explicit musical schema 2 and SQLite store schema 3; those are separate versions. Native takes are never resolved from arbitrary metadata. The prior SQLite schema 2 operation ledger remains intact. The sealed journal binds descriptor, interpretation headers, exact chunks, ordered prefix and retained interruption/index receipts.

The implemented boundary under review is:

1. Core explicitly upgrades a copy to musical schema 2. Only schema-2 `kind=recording` tracks own typed take clips. Old `recording`, `takeId`, `assetId` and vendor fields elsewhere remain opaque. Failed upgrades/appends must not partially mutate the original document.
2. A read-only sealed reader opens a fixed SQLite read transaction and validates the actual descriptor/manifest/prefix. Opening an unfinished journal must reject without marking it Interrupted or changing its owner/state.
3. A consistent backup becomes a durable owned immutable snapshot before main-store publication. Its physical SHA/byte count are distinct from the semantic manifest SHA. Backup failure may retain an identifiable staged object; it cannot publish a track.
4. `ingest_take` registers that verified owned object in its own committed transaction. A separate `publish_take` atomically publishes project/history/operation receipt. Publication failure rolls back those project changes while intentionally retaining the already committed registry and owned original; failed ingestion can retain an unregistered recoverable object. Operation retries return the original committed receipt rather than append a second clip or overwrite newer edits. Archive restore differs: all its registry/assets/project/history/import/operation metadata commits together after object staging.

Source paths are acquisition inputs, not durable object identities or permissions. An identical committed retry should resolve its retained receipt and owned object even after the external source moves. Different request content using the same operation ID must reject.

## Required invariants

| Boundary | Required check / independent scenario |
| --- | --- |
| Typed extraction | Inspect only the explicit schema-2 recording path. Preserve huge unknown integers/long decimals and foreign `recording`/`takeId` metadata across upgrade, publication, undo/redo and reopen |
| Take identity | Reference take ID and manifest hash must equal the opened sealed manifest; stream ID must exist in that descriptor; unsealed/wrong-manifest/wrong-take references reject |
| Selected prefix | Clip source range must fit the selected manifest's typed stream prefix, not retained unselected bad tails or a cached row. Source gaps stay explicit. Missing referenced chunks reject |
| Coordinates | AudioFrames and Events are distinct source ranges; Events counts are event ordinals, never an implied audio duration. Signed arrangement frame plus sample rate is separate from unsigned source positions and host ticks. Audio sample-rate mismatch requires rejection unless explicit resampling is implemented; event clips cannot acquire audio fades |
| Duration and size | Native metadata has its own bounded cap. Long native references must not inherit 30 MiB/48 MiB bridge-media caps or the 128-bar schema-1 timeline. Keep schema-1 import JSON/media limits unchanged while expanding native load/history/receipt guards |
| Locks and retries | Existing locked/non-recording target rejects; duplicate clip IDs have a defined identity scope. Same-ID same-content publication is idempotent through later edits/undo/redo; conflicting IDs, stale revisions and failed receipts leave no partial project/history/receipt. Previously ingested registry/original objects remain retained |
| Snapshot identity | Verify the actual opened transaction and the completed backup. Never infer identity from a basename, caller-provided hash or an earlier path check. Hash/close/fsync the owned file and parent before publication |
| Filesystem safety | Destination creation is exclusive and must reject existing/dangling symlinks without replacing their targets. The implemented OSAR SQLite archive has no extracted entry names; owned filenames derive only from validated content hashes. Source aliases may be allowed by the read-only API; bind their actual opened identity and do not mutate the original |
| Recovery retention | A recovered snapshot retains its complete interruption receipt, expected hash index and bad/unselected tail rows. Selecting a playable prefix does not authorize deleting or exporting only that prefix |

## Concrete per-project archive strategy adopted

The implemented archive is one standalone SQLite file, application ID `OSAR`, format version 1, with an exact supported schema. This avoids arbitrary extracted entry paths. Tables retain the project, both edit snapshots, operation requests/results/receipts, exact original import bytes, legacy assets, native registry metadata and complete sealed SQLite files encoded as ordered checksummed chunks. A filename, table name or opaque JSON field never grants arbitrary filesystem access.

**Freeze the graph and collect the closure.** One main-store read transaction selects the project and every retained history/operation/import row. Each bounded document is parsed and checked before export can succeed. Traverse explicit assets/takes in current state, both history sides, all operation results and exact original import JSON; unknown provider metadata remains opaque. Retain a take referenced only by an old operation after redo truncation. Include complete recovered files with interruption receipt, expected index and unselected bad tails, not only the playable prefix. Unassigned recovery objects require a separate explicit recovery selection/export policy.

**Bound and authenticate independently.** Project/history/result rows use the native 8 MiB limit; exact legacy import receipts retain 1,900,000 bytes. The archive separates a 256 MiB metadata budget, 1 MiB object chunks and a 1 TiB aggregate payload ceiling, with explicit count limits. These are format/resource ceilings, not a long-recording benchmark. Stream files and chunk validation; do not buffer a take. Validate per-row checksums, sequence/byte counts, complete physical object SHA and the semantic sealed identity. A logical metadata/index digest binds the selected closure. Limits must fail visibly instead of dropping retained rows.

**Publish without replacement.** Export uses a private exclusive staging directory and one checked archive transaction, then file/directory sync and atomic create-if-absent publication. Existing/dangling symlink destinations reject. Failure retains identified owned staging/orphans and never overwrites the live store or an existing archive. The trusted destination-parent scope does not promise protection against a hostile same-user process replacing its namespace.

**Restore in isolation.** A read-only held archive transaction validates application/version, exact schema, bounded rows/counts/budgets, checksums, reference closure and streamed object files before publishing a project. Malformed histories, unsupported request types, absent chunks and unrelated/missing rows reject. Stage owned immutable objects first; then assets, registry, import, project, history and operations publish in one main-store transaction. A later statement failure can leave recoverable files but no visible partial project. Reopen and exercise undo/redo and old receipt replay to qualify restoration beyond current-track readability.

Restore is currently identity-preserving. Unrelated projects are allowed; project IDs and global operation IDs cannot collide. Shared assets/takes require exact verified reuse. Copy-restore with remapped identities is explicitly pending: it requires a separately defined provenance transaction across every retained row and cannot be replaced by partial ID rewriting. No automatic GC follows export failure, removal of a current clip, or a retained orphan.

## Concrete findings during implementation

1. Archive request dispatch originally treated any top-level `kind` as a publication wrapper. A valid generic edit can carry opaque `kind`, `version` and `intent` extension names. The archive owner corrected dispatch to recognize the required generic intent identity first, then validate an exact typed publication wrapper. Independent regression uses `kind="take_publication"` on an ordinary edit to catch namespace takeover.
2. The first shared-save optimization checked only newly added take IDs. Publication preflight covered old IDs, but direct cached saves, generic edits and history navigation could commit while retaining a missing/corrupt old object. The coordinator chose strict validation of **all distinct takes in the result** before every commit. Shared save/history now perform that check once per distinct take; duplicate publication preflight was removed. A result that removes an unavailable take need not validate an unreferenced old object. Independent tests cover cached save, generic edit, undo and redo while the result still contains the damaged reference.
3. The existing project summary counted any track's `notes` array, although schema-2 recording tracks explicitly preserve that field as opaque. The coordinator excluded recording kinds from the note projection, and the app owner is adding a separate read-only recording branch. The independent store regression saves foreign `notes`/`audio`/asset-shaped recording metadata and requires no inferred note count or legacy asset dependency.

The focused store gate passed all 54 tests, including the fourteen publication and eight archive review tests. Core/recording passed 65 tests, including the six snapshot review tests. The final formatted workspace gate passed all 152 tests. Physical verification is linear storage-worker I/O; no realtime or verified-cache performance claim follows.

## Independent verification

The exercised APIs include `SealedJournal::open`, fixed-view descriptor/manifest/chunk access and `export_backup(new_destination) -> BackupReceipt`; read-only source aliases are intentionally allowed and destination aliases reject. Core's typed reference/range/position/clip, explicit upgrade/append/extraction, store ingestion/publication and archive export/restore are exercised through public APIs. A snapshot receipt's file digest remains distinct from the verified manifest digest.

The coordinator ran these gates; this review independently read the resulting logs:

| Check | Result | Evidence under `.runtime/rust-migration/` |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Exit 0 | `native-takes-format.log` |
| `cargo test --workspace --all-features --locked` | Exit 0; 152 passed, 0 failed/ignored | `native-takes-workspace-tests.log` |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Exit 0 | `native-takes-clippy.log` |
| Native archive qualifier, debug | Exact full payload/metadata restore and source unchanged | `native-takes-long-archive.log`; `docs/verification/rust-native-archive-roundtrip.json` |

The workspace total is app 12, core 22, IPC 21, recording 43 and store 54. The first workspace attempt exposed a test collector that read only interactive AccessKit labels; the corrected regression reads actual static `Role::Label` values and checks declared recording text, absence of legacy renderers and exact metadata retention. The passing run precedes a test-only clone-to-borrow assertion cleanup; final Clippy compiled the current all-target test sources. No production behavior changed in that cleanup.

Independent files are `crates/omniusong-recording/tests/snapshot_review.rs` (six tests), `crates/omniusong-store/tests/take_review.rs` (fourteen tests) and `crates/omniusong-store/tests/archive_review.rs` (eight tests): 28 new independent tests. They cover snapshots, publication/source binding/retries/fault rollback, and complete archive closure/restore/corruption/identity collisions. All fixtures are bounded synthetic UUID directories under ignored `.runtime`. No Cargo command was run by this lane; the coordinator owns the combined gate. Device capture, listening, callback timing, Windows, archive UI workflow and actual power-loss behavior remain outside this review's initial qualification.

The completed long-archive qualifier independently exercised 33 MiB PCM, 5,120 raw events and 113 chunks, with the native object retained only by operation receipts after redo truncation. Its archive restored four operations and one history row, exact original payload and opaque metadata; the source file was unchanged. The actual receipt is `docs/verification/rust-native-archive-roundtrip.json` and log is `.runtime/rust-migration/native-takes-long-archive.log`. The debug process took 58.49 seconds with 21,839,872 bytes maximum RSS. This is a bounded synthetic storage experiment, not elapsed capture, production throughput or a latency result.

All APIs here perform blocking storage/validation work. Strict physical verification reads every distinct referenced take linearly, and the UI background storage worker for these actions is **not integrated**. Library snapshots/ingestion/publication/archive/identity-preserving restore do not establish an end-to-end UI workflow. Copy-restore, live audio/MIDI producer/scheduler, callback safety, permissions, actual power-loss/disk-full behavior, Windows and the thirty-minute real-device stability gate remain open.

The read-only recording branch is qualified by its headless frame regression only. `prepare_native_ui_fixture.rs` was source-reviewed and hashed below; this review does not claim that example or a new native window was exercised. Its generated four-frame source is explicitly synthetic, never captured or played audio.

## Final source snapshot

SHA-256 after formatting and the final test-only Clippy cleanup, October 20, 2026, 18:38 UTC. The earlier operation-review file changed only for the authorized SQLite version-3 expectations and native-vs-legacy byte-cap fixtures; all ten of its tests pass.

```text
57ac6420c95fb750e903a26c21b68bcee8ececfdfd0f5c7d071930bda1018448  crates/omniusong-core/src/lib.rs
e5c65a632a7e9520ade21c8daf3b8ae786769e83af543a24a15db83b7a53d549  crates/omniusong-core/src/native.rs
d3c5812bc3d7c9c8c96c17c841c9755ccadde535648d8559a00520a53048e889  crates/omniusong-store/src/lib.rs
906f6ef0ea1ef05f73d500cef8d77fb9288cbe8c3a40682b00161575ac15ee22  crates/omniusong-store/src/operations.rs
999183c1b01c940264b231b9b61fce36b76554db94c6c032ea24d25033c2c77a  crates/omniusong-store/src/takes.rs
4a5a09ff57d69c2480159649810f5c9d0081f13e398cbef11e8c2bb8cc9ca29b  crates/omniusong-store/src/archive.rs
bb20e1caf13aa6510870912a8629d860d698c17aadae1e0e76642f65b06131b6  crates/omniusong-recording/src/lib.rs
8f874682f31e9d82d8f2b325ed8033089865cbf61ac066e8ec7d5b1d32dfddc4  crates/omniusong-recording/src/journal.rs
4de4f55c613a447a24b876b0c7f2b2097fb2f42b94df0a7be5e5318d6971b8a5  crates/omniusong-recording/src/types.rs
95608256e073a42765ffe09aae7af63d12fd3c42eb0d0766b527ff9b5d8cd8c8  crates/omniusong-recording/src/snapshot.rs
a8861732ef7eb9ddc44b10c4157ce5de47dc60cc57ad0d7f9b20db2dc8ef5cde  crates/omniusong-app/src/main.rs
520890fb8b5226a46666460f09d35ecb0f3c5c42a9bb91bfb1b8860a36b64da4  crates/omniusong-store/examples/prepare_native_ui_fixture.rs
dff317f6b8005192f8c5cfe716d13d2bf0fd5dbc8ba1943436ad91a503dbca7f  crates/omniusong-store/tests/take_review.rs
10cf9e14a7b077fbe37df6868c11c249e99d26c003d5c27bbc92c95092f216d1  crates/omniusong-store/tests/archive_review.rs
8a8ac2d7b4ccd5977968cff469b229e51c57a5e76b2161b44cc64161c50482f0  crates/omniusong-recording/tests/snapshot_review.rs
ca0da657bec5110fccfa86fca24ab67df1380e56f88a683c53041ef6064c5a4b  crates/omniusong-store/tests/operation_review.rs
```
