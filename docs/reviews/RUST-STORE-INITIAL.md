# Initial independent Rust persistence review

October 20, 2026. Scope: `omniusong-core`, `omniusong-store`, root dependency configuration, and the native store API's use at application startup. The initial probes found active-document metadata loss, unchecked edit-history identity, incomplete clip validation, an import-metadata collision, and a library-list recovery problem.

**Current disposition, October 20 at 17:27 UTC: F1–F5 are verified fixed for the stated regression scenarios.** Source changes were inspected, six independent integration regressions passed in the fresh workspace gate, and the reviewer independently reran the current core/store/integration test executables with 28 passing tests. The findings below retain the original failure evidence as history. This closes this bounded initial persistence review; it does not constitute full migration or release acceptance.

## Verified disposition and current evidence

| Finding | Verified correction | Independent regression evidence |
| --- | --- | --- |
| F1 | History snapshots must have the target project ID before an update can commit | Both undo and redo receive foreign-ID but otherwise valid snapshots. Each returns `StoreError::Invalid`, leaves the full payload/revision/cursor/edit journal unchanged and permits reopening the intended project. |
| F2 | `serde_json/arbitrary_precision` retains the unknown literal numeric values | A real legacy JSON file enters `import_legacy`, then rename/save, undo, redo and multiple store reopenings. The integer `18446744073709551617` and decimal `0.12345678901234567890123456789` remain exact; the database's original JSON receipt and source file remain byte-identical. |
| F3 | Clip windows, fades, reconstructed original settings, transport and legacy recording timing are validated | Eight invalid clip/snapshot/transport/timing imports are rejected with zero project/asset/import/edit rows. A valid moved/trimmed/faded clip and its original snapshot survive import/reopen with their asset bytes intact. Native audible clip restoration is not implemented or qualified by this test. |
| F4 | An existing `nativeImport` value is retained under `previousImport` | The real import/edit/history/reopen test keeps prior lineage and opaque metadata, then imports another independent native copy and verifies the complete previous receipt object and exact second-source bytes. |
| F5 | Listing returns a damaged-session summary with `problem`, alongside healthy summaries; identity/revision are checked | Malformed JSON, payload-ID mismatch and row/payload revision mismatch remain visible as damaged entries. A healthy session remains listable/loadable, and all row payloads/revisions/cursors/journals remain unchanged after listing and reopening. |

The asset-reference correction is also verified: foreign namespaced metadata may contain a remote string `assetId`, numeric `sourceAssetId`, and a string shaped like a local hash filename without requiring or importing a local blob. Only the explicit schema-owned reference paths are collected. The integration test imports this metadata from literal JSON without supplying any asset and checks that the metadata stays intact and the asset table stays empty. Genuine schema-owned audio references are exercised by the positive/negative clip and transaction fixtures.

The final-import-statement regression now runs against the current implementation: a SQLite trigger aborts the receipt insert after project/asset writes; the entire new import rolls back, the earlier project reopens, and source bytes remain unchanged. The coordinator additionally fixed the first-open schema race by rereading `user_version` while holding the immediate migration transaction. Its four-simultaneous-initial-open unit regression passed in the inspected gate and in the reviewer's direct rerun.

Fresh evidence:

- Coordinator gate: `.runtime/rust-migration/native-workspace-tests.log` — 37 passed, 0 failed, 0 ignored: 9 app, 14 core (including 7 contracts), 8 store and 6 review integration tests. The reviewer inspected the complete finished log; no competing Cargo build was started.
- Independent execution of the freshly built binaries: `.runtime/rust-store-review-20261020/current-core-tests.log` (14 passed), `current-store-tests.log` (8 passed), and `current-integration-tests.log` (6 passed). All three commands exited zero. The contract test count is reported as executed evidence; this review does not independently approve every audio/MIDI/worker contract's semantics.
- Regression source: `crates/omniusong-store/tests/review_regressions.rs`. It uses only public store/core APIs plus deliberately injected SQL faults in synthetic databases under `.runtime/review-regressions/<uuid>/`. Successful fixtures clean themselves up; failed fixtures retain their path for inspection. No legacy source project or asset is an input to these destructive test cases.
- The integration binary is `target/debug/deps/review_regressions-45ea9d297e505048`, SHA256 `617798be70d929ccd9634d7e07ab7562c95d693470ee4d6429afe5e52d91b5d2`. Current regression source SHA256: `43fa472f76fcd8f31ec2c9d46c463f410290aa4d2cf13408c8e83704200b2402`.
- Reviewed current core source SHA256: `92d19c5f97d253a219f42dc66d708209b6372b3745d0e0f087c4c59bc98e6dae`; store source: `6a83c198003ff324d957cd95e5eb54198b401c1e0ab2f137000c087feae20cef`; root manifest: `fb7932529c13850097a7da69ea0d0900f551244a327e93f90bd212b53e69f8fd`; lockfile: `395ebcaa0d0ea860368c4c489340dc5aba5785e4a5203b69e6b1d8bed94c5643`.

Remaining limits: these are bounded synthetic metadata/blob and SQLite transaction checks. They do not establish valid audio decoding or listening quality, real note/clip editing UI, physical microphone/controller acceptance, capture-fragment recovery, process-kill/power-loss/disk-full recovery, portable archive completeness, candidate acceptance/idempotency, or Windows behavior. The native GUI and operating-system permission gates remain separate. No additional unresolved correctness finding arose in this bounded fix-verification pass.

## Initial findings and original failure evidence

### F1 — P1: undo can commit a different project's identity

Initial location: `crates/omniusong-store/src/lib.rs`, `navigate_history`, originally lines 344–356.

The original implementation parsed `before_json`/`after_json`, changed its revision and wrote it into the requested project's row without checking the snapshot's ID. A valid JSON snapshot with a changed ID therefore passed document validation, returned success from undo, and left the row inconsistent. The next `load(original_id)` rejected it as an identity/revision mismatch, so ordinary reopening could no longer recover that native session.

Reproduction: create and save project A; close the store; in the isolated test database replace edit sequence 1's `before_json` with another valid project B; reopen and undo A at revision 1. The probe returned B successfully, then A failed to load. This is an explicit stored-history fault scenario, not evidence that normal `save` writes another project's journal.

Required regression: exercise both undo and redo with an otherwise valid foreign-ID snapshot, assert a typed error, unchanged project payload/revision/cursor and unchanged history, then reopen and load A successfully. The coordinator's added `next.id() != id` check addresses the observed publication flaw; its test result was not available when this receipt was written.

### F2 — P2: unknown numeric metadata silently loses precision

Initial locations: root `Cargo.toml` `serde_json` dependency; `crates/omniusong-core/src/lib.rs` `ProjectDocument::from_json` and `to_json`.

The initial dependency omitted `arbitrary_precision`. Parsing otherwise valid unknown JSON numbers into `Value` rounded numbers beyond `u64` and long decimals. The probe imported an extension object containing integer `18446744073709551617` and decimal `0.12345678901234567890123456789`; the active native document contained `1.8446744073709552e+19` and `0.12345678901234568`. Saving/reopening that document preserves the rounded values, not the source values.

The original JSON receipt stayed byte-for-byte intact, which provides a recovery source but does not make the active document lossless. This does not show that the 19 existing source projects contain such numbers; it demonstrates a violated unknown-metadata boundary for valid inputs.

Required regression: import literal JSON containing a number above `u64::MAX`, a long decimal and unknown nested namespace fields. Rename/save, undo, redo and reopen, then verify the exact opaque numeric values/representations and the retained original bytes. Constructing the fixture with an already-rounded `f64` or comparing two independently rounded `Value`s cannot catch the defect. The coordinator has enabled the needed feature; fresh artifact verification remains required.

### F3 — P2: native schema validation accepts impossible clip settings and broken originals

Location: `crates/omniusong-core/src/lib.rs`, audio validation within `ProjectDocument::validate`, originally lines 338–359.

The native validator checks source duration, MIME, byte length and optional start tick, but omits the existing schema-1 clip-window and original-clip rules. It accepts negative trim starts, trim ends beyond the complete source, negative/overlapping fades, and an invalid `audio.editSource` version/original snapshot. `NativeStore::create` then publishes that document successfully. These are recognized musical fields, not opaque extension data.

The isolated probe published an audio clip with one-second source duration, `trimStartSeconds: -100`, `trimEndSeconds: 500`, `fadeInSeconds: -1` and `editSource.version: 77`. The legacy validator rejects these through `clipWindow` and `originalClipAudio`. A future renderer/editor expecting schema-1 invariants can fail after an apparently successful native import/save; original-clip restoration cannot be trusted for the malformed snapshot.

Required regression: port the established clip-window and original-snapshot constraints, including trim bounds/minimum duration, finite values, combined fades, allowed snapshot keys/version and valid reconstructed original. Test malformed import rejection without publishing any project/assets/receipt, plus a valid moved/trimmed/faded clip whose complete original restores after save/reopen. Recording-placement and transport metadata validation are also absent from the native projection and need explicit treatment before those behaviors are enabled.

### F4 — P2: importing overwrites an existing `nativeImport` field

Location: `crates/omniusong-core/src/lib.rs`, `ProjectDocument::imported_copy`, originally lines 247–253.

`imported_copy` unconditionally assigns the top-level `nativeImport` property. An existing field of that name is removed from the active copy, whether it contains an earlier import lineage or previously unknown source metadata. The probe constructed a valid document with `nativeImport: {"provider:original": {"exact": "must remain"}}`; `imported_copy` lost that object. Raw imported bytes remain available separately, but ordinary document round trips no longer retain the field.

Required regression: import a source that already has `nativeImport`, save/reopen and import a further copy. Preserve prior content/lineage explicitly, or keep migration provenance in the store's import ledger without replacing a source field. State the identity/provenance transformations in the schema; do not claim arbitrary top-level metadata survives if it can collide with a newly introduced field.

### F5 — P2: one damaged session prevents listing every healthy session

Locations: `crates/omniusong-store/src/lib.rs` `NativeStore::list`, originally lines 168–188; `crates/omniusong-app/src/main.rs` `StudioApp::open` calls `store.list()?`.

`list` parses every payload and collects into one `Result`; any damaged project's JSON aborts the entire list. Application startup propagates that error, so one bad session blocks the native UI from opening the other healthy sessions. In the probe, two sessions were committed, one payload was replaced with malformed JSON, and `list()` failed while direct loading of the healthy ID still succeeded.

This is an explicit database fault test. It does not show physical database-page recovery, but demonstrates that even a localized payload failure has library-wide availability consequences.

Required regression: retain a visible damaged-session entry/error and permit listing/opening healthy rows, without silently rewriting or dropping the damaged payload. Check row identity/revision when building summaries as well as when loading. Reopen with one damaged row and assert healthy-session access and an actionable recovery path. A separate destructive repair decision should not be hidden inside ordinary listing.

## What the review established positively

The normal save path obtains an immediate SQLite transaction before checking the stored revision, writes the journal and project update together, and truncates redo history in that transaction when creating a new branch. The existing sequential two-connection test covers stale optimistic writes. Document views clone/preserve fields for rename and tempo updates; raw capture timestamps are strings and supported note/expression integers retain their resolution.

Legacy import validates bounded project input and content-addressed referenced assets before publication. The project, asset blobs and exact original JSON receipt are committed in one transaction. Missing or wrong-checksum input is rejected, and the native store is separate from the legacy file source. Asset references found in provenance/history are included, rather than copying only the visible audio tracks.

An additional isolated SQL-trigger probe forced failure at the final import-receipt insert, after the asset/project statements had run. The operation failed, all newly written assets/project/receipt rows rolled back, and the previously committed healthy project still loaded. The source JSON and source asset were unchanged. This is stronger evidence of the implemented import transaction than only failing during pre-validation. It still does not simulate process death, storage failure or power loss.

## Test evidence and limits

The initial `.runtime/rust-migration/native-core-tests.log` reports **3 core + 5 store tests passing**, with no ignored tests. It was inspected directly. At review start the workspace had only core/store members; the coordinator was adding the app member and rebuilding dependencies. This review did not run a competing Cargo build. Instead, small standalone Rust probes linked the existing test-build libraries and wrote only an isolated runtime folder.

The eight tests cover:

- Unknown object metadata, a `u64::MAX` integer, decimal `0.75`, raw UMP data and large timestamp strings; they do not cover arbitrary-precision JSON numbers, field-name collisions or every supported musical field.
- Unsupported schema versions, one out-of-range expression point, invalid rename/tempo and one malformed asset-path reference; they do not establish complete schema-1 audio/transport/recording validation.
- Rename/tempo save, reopen, undo, reopen, redo and a new edit branch; they do not test note/clip/expression edits, candidate acceptance, locks, input capture mutation or imported audio through durable undo.
- Sequential stale writes from two open connections; they do not coordinate simultaneous commit races or sustained concurrent writers.
- One imported fake audio byte string and exact source JSON bytes; it proves content-addressed blob preservation, not a valid audio decoder, sample count, clip duration or audible result.
- Missing/wrong-checksum assets rejected before publication; they do not inject failure at each import statement. The extra trigger probe above covers one later failure boundary.
- Dropping a transaction with an uncommitted manual update and reopening; it is not kill/crash/power-cut/disk-full/torn-page fault injection and does not by itself prove capture recovery or physical durability.

No test in that baseline covers undo/redo with damaged-but-parseable snapshots, a corrupted active row's effect on library access, or source import containing a preexisting `nativeImport` field. No native portable archive, incremental recording journal, candidate transaction or model output acceptance path exists in these two crates yet; those release requirements remain pending.

## Reproducibility

Artifacts are local and ignored under `.runtime/rust-store-review-20261020/`:

| Artifact | Purpose |
| --- | --- |
| `probe.rs`, `probe`, `probe-results.txt` | Reproduces the five initial findings without modifying legacy data |
| `fault-probe.rs`, `fault-probe`, `fault-results.txt` | Final-import-statement failure and rollback/source-preservation probe |
| `probe-data/`, `fault-data/` | Independent artificial JSON/assets/SQLite databases |

The initial probes linked `libomniusong_core-a91c483c330a0dfc.rlib`, `libomniusong_store-7848b71d842e46b0.rlib`, `libserde_json-0cf7bf810ef0385a.rlib` and `librusqlite-4b7a012f88a3be19.rlib` from the initial test build. Probe SHA256: `8b87a03e93db0d7bd2233b82529cfde88a4e6e1d36dc686da07168b7b50e4a5b`; source SHA256: `4efcb02f247ba2ddcf808b9bdde9c9e210a1bb2ff7417a72a8274f3b38985b0c`; result SHA256: `ade3402931bfe4ce686fa6de4b8cfd188f94e837eef217afa32349be4289ca68`. Rebuild probes against newly compiled libraries for fix verification; do not mistake an old binary for the current source result.

The reviewer wrote this document, the separate `crates/omniusong-store/tests/review_regressions.rs` integration test, and isolated probe/log files. Shared implementation source, manifests and legacy data were not edited by the reviewer. The 19 legacy projects and 11 baseline assets were not modified by these tests. No model work, downloads, recording uploads or runtime repairs were performed in this review.
