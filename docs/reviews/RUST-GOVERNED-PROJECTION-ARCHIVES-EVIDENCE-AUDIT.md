# Governed projection archives: evidence audit

Date: 2026-10-22, America/Los_Angeles. **Decision: bounded backend acceptance; passed: true.** The final evidence supports the three new governed OSAR APIs together. It does not establish native workflow, Windows, packaging or full workstation acceptance.

I read the contract, relevant Rust/test source, final packet, raw qualified-gate logs, the two explicitly allowed provenance scripts and preservation snapshot JSON. I used read-only JSON, SHA-256 and test-row checks; neither provenance script was executed. I ran no Cargo, formatter, tests, SQL, application or device operation. Only this audit document was written. No catalog, installed executable or protected object referenced by a preservation record was opened or hashed by this auditor.

## Frozen execution evidence

The [packet](../verification/rust-native-governed-projection-archives-tests.json) is **75,993 bytes**, SHA-256 `5f2e771de302d442767b3f31d99753b6731f57ca331ce82df5ace8d6103a1071`. The [qualified gate receipt](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-run.json) is **112,898 bytes**, SHA-256 `31471421c22f61a7b6c690c804a16a13c0d23a5efd7757ecd0a6259ce806051b`.

All **280** gated input fingerprints match current files and the packet's exact before/after inventories. All **244** current crate Rust/JSON/TOML inputs are included. The separately bound source review, contract, nine raw logs, gate receipt, runner and packet-builder fingerprints also match. This establishes stability of the declared gate inputs, not a claim that every repository document was frozen. Historical candidate/preceding-stage references and preliminary transcript summaries were not followed or independently revalidated; they supply no final acceptance totals here.

The packet's nine command records exactly equal the gate receipt, with exit zero throughout. The inspected runner captures actual exits/raw output, uses serial test arguments, stops on failure or an empty test selection, and compares source inventories. I independently recomputed the following from raw `ok`/`FAILED` summary lines and matched the packet:

| Qualified raw execution | Passed | Failed | Ignored | Groups |
| --- | ---: | ---: | ---: | ---: |
| [Workspace, all features](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-workspace.log) | 1,443 | 0 | 9 | 72 |
| [Optimized store library](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-store-library-release.log) | 275 | 0 | 1 | 1 |
| [Optimized session library, all features](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-session-library-release.log) | 123 | 0 | 2 | 1 |
| [Optimized engine projection](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-engine-projection-release.log) | 61 | 0 | 0 | 1 |
| [Optimized archive review](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-archive-review-release.log) | 8 | 0 | 0 | 1 |
| [Optimized archive-copy review](../../.runtime/rust-governed-projection-archives/gate-qualified/gate-archive-copy-review-release.log) | 10 | 0 | 0 | 1 |

The selected optimized total is **477 passed**, not 477 new cases. Formatting, warnings-denied workspace/all-target/all-feature Clippy and the default-feature all-target workspace build also passed. Every test command includes `--test-threads=1`; this does not qualify a parallel harness. Ignored cases are excluded from passes.

All **56 packet-listed new case names** have actual test attributes in current source and occur exactly once as successful workspace rows. **55** also occur exactly once in the selected optimized logs; the performance-crate finalizer unit case is workspace-only.

## What the cases establish

The [eleven governed engine cases](../../crates/omniusong-engine/tests/performance_projection_review/governed_archive_review.rs) use actual device-free `PerformanceWriter` profiles v1/v2, recover interrupted originals, authenticate them and publish through the real typed store API. They also use a separate recovered `CaptureJournal` ordinary audio original. The legacy asset bytes are hash-addressed synthetic data, without a WAV-decoding claim.

Public export, identity restore, two copy generations and re-export preserve original registrations/manifests/raw bytes, exact projection registries and deterministic publication/mapping rows. Copied document values are compared after normalizing the owned project ID. Source all-table/schema snapshots remain unchanged; full catalog snapshots also establish exact rollback in adversarial cases. Hidden/pruned operation results and actual copy ancestry remain represented. The strict **OSAR-v1** case removes the exported archive's lineage tables and sets its format version, then exercises both public restore modes for both performance profiles; performance-record v1 alone is not treated as archive-format compatibility evidence.

The negative scopes are concrete: coherent false music and a wrong pruned initial muted lane refuse bounded **export**; registration replacement and unrelated-asset triggers refuse **both restores**; an existing wrong owned object refuses identity restore. The [store final-boundary control](../../crates/omniusong-store/src/archive_governed/tests.rs) replaces an ordinary-audio owned pathname with identical bytes at a different inode for **all three routes**, checks refusal/rollback and clean retry. These are distinct controls, not every negative on every route/profile. Exact replay succeeds without input, preserves later human edits and retains `RecordedReplay` on a missing-owned-original refusal. Typed cancellation/work/VM stops, cumulative credit, healthy-store reuse and late successful publication are exercised.

The [copy materialization controls](../../crates/omniusong-store/src/archive_copy.rs) deny digest-column authorization to prove that a starved pass stops before preparing its payload-bearing point query. Metadata tests check prepayment before damaged large JSON and exact legacy index/totals/raw-row parity. These controls do not measure every internal SQLite allocation. Seeded binding-memo tests qualify exact-byte and namespace bookkeeping only; they do not authenticate a performance original or count source authentications.

Error evidence distinguishes real private NOTADB/FULL and live trigger, progress, update, commit and authorizer behavior from deliberately constructed SQLite-code/nested-taint controls. The latter establish classification and retention, not physical main-catalog corruption. Governed boundaries retain received errors beside a separate sticky stop. The recording layer can still replace a lower-level error before those boundaries receive it; universal physical-error preservation is not established.

## Preservation and remaining scope

The explicitly allowed [before](../../.runtime/rust-seal-native/governed-archives-before-tests.json) and [after](../../.runtime/rust-seal-native/governed-archives-after-tests.json) snapshots are each **51,310 bytes** and match their packet fingerprints. I compared exactly `tables`, `projectIds`, `files`, `catalogFile` and `binary`: all equal. They contain **13 tables, 22 project IDs, 403 rows and 80 protected-file records**. The catalog record remains 8,613,888 bytes at SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`; the installed executable record remains 23,062,544 bytes at `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. These are snapshot-content comparisons; target verification was the coordinator's operation. Schema 6 is carried forward by exact catalog bytes, not a fresh auditor SQL query.

The [contract](../contracts/NATIVE-GOVERNED-PROJECTION-ARCHIVES.md) and [conditional source review](RUST-GOVERNED-PROJECTION-ARCHIVE-REVIEW.md) remain consistent with this bounded result. Legacy nonempty-projection archive guards remain closed, and native/session dispatch has not switched. Schema-3 inert OSRE, guarded reader/list/attachment integration, durable native admission, warning transport and named-binary edit/history/archive/restart/audition still require separate work. No one-auth count, every-allocation or wall-clock bound, hostile-directory/power-loss guarantee, physical input/output/listening, long capacity, new installed binary, UI, Windows or full release acceptance is implied. The complete goal and milestone 2 remain open.
