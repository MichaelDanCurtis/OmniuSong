# Ordinary archive outcomes — independent source review

## Decision and scope

**Bounded source acceptance: no unresolved correctness blocker found in this slice.** Reviewed the working tree based on `c5ea82849b9e7386c7059f8a6e2182ba7f4b81c7`, including the final separately requested/observed archive fingerprint fields and boxed failure context. The implementation gives the existing ordinary OSAR export, identity restore, restore-as-copy and header inspection routes explicit completion, failure origin and cleanup semantics. It does not activate projection archives.

This is source review, not independent execution or a final evidence audit. The reviewer read source, diffs, test definitions and the [archive outcomes contract](../contracts/NATIVE-ARCHIVE-OUTCOMES.md). The reviewer ran no Cargo command, formatter, build, test, SQL, application or device operation, and opened no default/protected database or recording. Only this review document was written. Final gate counts, stable-source fingerprints, default-store preservation and installed-binary preservation remain for the separately recorded execution packet.

## Completion boundaries

The four existing APIs delegate to the new outcome APIs. Successful `ArchiveOutcome<T>` preserves the previous value and adds a completion plus concrete private-cleanup issues. Failure retains its original `StoreError`, observed phase, source/destination/staging paths, copy operation identity, requested archive hash, observed archive hash and confirmed object-placement count. `ArchiveFailure::Display` retains this context for existing callers that store an error string. Its boxed context is a representation choice, not a reduction of information. Cleanup helpers are crate-private.

In [archive.rs](../../crates/omniusong-store/src/archive.rs), export prepares the receipt after private archive commit/close, file synchronization and whole-file hash, before the hard-link fence. `OutputLinkAttempted` is set before the call, `OutputLinked` immediately after success, and `ExportDurable` only after destination-parent synchronization. A link or parent-sync error retains the completed private file and its observed digest without claiming durability. After durable publication, removal of the known private file and then the empty staging directory can add cleanup warnings but cannot turn the result into failure. Unexpected entries prevent empty-directory removal and are retained.

Identity restore builds its project result before the sole catalog commit. [archive_copy.rs](../../crates/omniusong-store/src/archive_copy.rs) likewise validates and constructs the full copy receipt before commit. Both mark `CatalogCommitAttempted` before entering the fence and `CatalogCommitted` immediately after it succeeds. No source validation, live SQL or fallible receipt construction follows successful commit; only private connection release, outcome construction and private staging cleanup remain. The result therefore cannot falsely suggest rollback solely because cleanup failed.

`ObjectsPlacementAttempted` is recorded before installation and the counter advances only after a confirmed hard link. A later SQL rollback does not remove content-addressed object files. An error with zero confirmed placements is not proof of no filesystem effects. Retained orphans and staging are deliberately not adopted or repaired by this change.

Requested and observed archive hashes have distinct meanings. Copy records the expected SHA only after intent validation and the observed SHA only after successful acquisition and comparison. Export records its observed output hash before linking; inspection records the acquired input hash before SQLite inspection. A failed acquisition does not fabricate an observed authenticated hash. Identity restore continues its existing path-based validation without adding an unrequested whole-file hashing pass.

## SQL origin and transaction cleanup

[archive_boundary.rs](../../crates/omniusong-store/src/archive_boundary.rs) distinguishes `Input`, `PrivateOutput` and `LiveCatalog`. The wrapping helper changes only a bare `StoreError::Database`; it preserves prior SQL tags, `CatalogFailure` and nested archive context. Private SQLite fatal primary codes remain diagnostics and do not become a claim about the studio catalog.

The review traced the mixed-handle operations rather than relying on outer route names:

- `Destination::sql` tags destination project/history/operation/import writes in `copy_metadata`.
- `read_lineage` receives an explicit destination origin and tags every destination proof, mapping and association query/write. Its source errors remain available to the appropriate source wrapper.
- Export tags private asset/chunk/registration writes separately from live source reads. Its separately opened read-only `studio.sqlite3` is explicitly live-catalog I/O.
- Identity-restore collision checks and object installation distinguish private archive reads from actual destination queries, asset writes and take registration.
- Copy input cursors and row conversions are explicitly input-local; destination SQL, temporary mapping SQL and final live validation remain live. The remapper distinguishes destination collision checks from input collision checks, including the version-1 archive branch without lineage tables.

Every actual live-catalog transaction in these routes now uses [CatalogTransaction](../../crates/omniusong-store/src/catalog_transaction.rs). The explicit legacy constructors install no governor or progress hook. Successful read scopes roll back explicitly; ordinary errors use the existing cleanup path, which retains the original error, cleanup SQL errors and remaining transaction state. A private-input primary error cannot hide a failed live rollback. Conversely, an outer private wrapper cannot relabel that cleanup failure as private SQLite. Panic-time `Drop` remains a best-effort backstop, not a structured returned failure guarantee.

[StoreError](../../crates/omniusong-store/src/lib.rs) recursively exposes actual live-catalog codes and unusable-connection state. The [session mapper](../../crates/omniusong-session/src/lib.rs) uses those facts for retirement and ORs archive publication uncertainty with the existing command policy. Actual live IOERR/CORRUPT/NOTADB primary codes remain fatal; nonfatal rollback failure can independently make a connection unusable. Input/private-output codes alone do not retire a healthy generation. The evidence diagnostic mapper preserves explicit nested archive SQL provenance; this is compatibility handling, not a new ordinary-archive evidence-job route.

## Replay and authority

Copy's early replay occurs before input acquisition or new identity allocation. It checks the exact operation/archive pin against the retained proof, validates the existing owned closure and constructs the historical receipt inside a live read transaction. Explicit rollback precedes `RecordedReplay`. The second replay fence inside the writer transaction follows the same rule. Original-input absence therefore does not force a new copy or new IDs after a recorded successful copy. Replay still depends on the existing destination closure being usable; this change does not create an independent compact commitment-fact API for an unavailable destination.

Attempted link/commit failures remain conservatively uncertain even where an injected test observes rollback. The new context is an in-memory account of this attempt, not a durable job journal or crash-reconciliation record. It neither authorizes blind retry nor resolves an acknowledgment lost across process termination. Identity restore remains conflict-based, and export remains a no-overwrite publication.

The four compatibility APIs intentionally discard successful cleanup warnings because their return types have no warning field. Their typed errors preserve provenance and context. Existing session/native consumers therefore gain correct failure classification but do not yet display successful cleanup warnings. No native workflow acceptance follows from this library change.

## Tests inspected and execution status

The reviewed test definitions include seven [archive outcome cases](../../crates/omniusong-store/src/archive/outcome_tests.rs), five added copy/inspection cases in [archive_copy.rs](../../crates/omniusong-store/src/archive_copy.rs), four [boundary cases](../../crates/omniusong-store/src/archive_boundary/tests.rs), three added [legacy catalog-scope cases](../../crates/omniusong-store/src/catalog_transaction/tests.rs), and three session/diagnostic cases across the session library and evidence-job tests. Existing ordinary archive, copy-of-copy, identity, exact replay, concurrency, collision and rollback regressions remain relevant.

Their consequential distinctions are sound:

- Ordinary export and identity restore compare actual receipts, documents and retained rows. The unexpected-entry export test causes a real filesystem `remove_dir` failure after durability.
- Boundary/cleanup hooks inject deterministic I/O failures; they do not simulate physical media failure or power loss. Tests check linked versus not-yet-linked output, retained stage/hash, commit-attempt phase and reusable destination state.
- Copy tests compare complete catalog cell snapshots and input bytes around refusal; a real SQLite commit hook rejects the actual commit and verifies rollback followed by reuse.
- Malformed private bytes exercise real NOTADB on inspection/copy/restore. The session test then creates a project on the same healthy generation.
- Mixed metadata helpers use actual missing destination tables to check origin preservation. The fatal-code matrices are explicitly constructor classification, not physical corruption tests.
- The denied-rollback test makes a real pending SQLite mutation and denies actual ROLLBACK. Its private primary error is constructed. The assertions correctly separate that constructed input error from the real cleanup failure and active-transaction state.

At review freeze, the root reported an initial focused archive result of **108 passed, 1 failed, 1 ignored**: an older lease-error assertion expected a bare error where the compatibility API now retains typed archive context. Root reported correcting that exact assertion after undoing an initial edit to a neighboring ingest test, and a focused cross-root rerun of **1 passed, 0 failed**. Root also reported **6 passed, 0 failed** in the session-focused run. An earlier test compilation diagnostic used nonexistent `TransactionOperation::Commit`; the final test correctly matches the pinned rusqlite authorizer's `Unknown` representation for COMMIT. The first full gate subsequently stopped at Clippy's test-event common-prefix warning. The final test-only variants are `OutputLink`, `ExportSync`, `RestoreCommit`, `CopyCommit` and `PrivateCleanup`, with their same pre-call positions; the corrected full gate was pending when this document froze. These are root-reported intermediate diagnostics, not reviewer-run or independently log-audited acceptance. No aggregate pass is claimed here.

## Limits and next integration

The [contract](../contracts/NATIVE-ARCHIVE-OUTCOMES.md) accurately keeps nonempty projection admission closed. `Bundle::references`, `Proof::document`, generic reference readers and the explicit typed-projection arm of archive request validation still refuse unsupported projection authority. The retained source-semantic validator is not wired into these public routes. No receipt/hash alone becomes source proof.

Existing archive row, byte and lineage ceilings remain, but these legacy route scopes do not provide cumulative `InspectionBudget` accounting, archive VM quotas or responsive cancellation. Filesystem calls, busy waits, synchronization, commit and rollback have no new hard wall-clock bound. The presence of the shared transaction wrapper must not be described as archive governor qualification.

This slice also does not establish exact expected destination table postimages against arbitrary side-effecting triggers, governed held-source lifetime through installation, final installed-path performance authentication, or complete projection export/identity/copy semantics. Those coherent route integrations remain necessary before opening projection archive guards. Header inspection remains a summary, not complete hidden-history/proof/source validation.

Guarded reader/list/attachment integration, schema-3 inert OSRE roundtrip, durable native admission, successful-warning presentation and a named-binary native walkthrough remain open. There is no new application package, native input/output, playback/listening, long-session, Windows, AI-inference or complete-milestone claim. The full workstation goal is unchanged.

## Addendum — retained namespace-refusal assertion

The root reported that the second full gate stopped in the existing engine test `fresh_generic_paths_cannot_claim_projection_namespace_but_exact_legacy_receipts_still_replay`: its archive-copy assertion still expected a bare `StoreError::Invalid`. The reported partial aggregate was **646 passed, 1 failed, 4 ignored, 17 groups**; it is retained as a failed intermediate diagnostic, not final qualification.

Reviewed the precise correction in [publication_review.rs](../../crates/omniusong-engine/tests/performance_projection_review/publication_review.rs). The test now calls `restore_archive_as_copy_outcome`, requires its retained operation error to be `Invalid` with the existing projection refusal reason, and additionally requires `NotStarted`, no staging path, no observed archive hash and the exact requested SHA pin. The nonexistent input path, unchanged catalog/source-byte assertions, adjacent generic-path refusals and later exact historical-replay checks remain. This adapts the oracle to the new typed boundary and strengthens proof that namespace refusal precedes archive acquisition; it does not relax the refusal or change production behavior. No new source blocker was found. The reviewer executed no test. The corrected complete gate, including the root's added selected release engine coverage, remains pending separate evidence qualification.
