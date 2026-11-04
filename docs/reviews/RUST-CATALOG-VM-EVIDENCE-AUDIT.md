# Scoped catalog VM: independent evidence audit

Date: 2026-10-22. **Decision: bounded acceptance; passed: true.** No missing or mismatched evidence reference, count, current qualified source input, or preservation comparison remains. This accepts the selected Rust catalog-governance prerequisite. It does not accept a new native package or the complete projection workflow.

I read and hashed the actual artifacts, source files and raw logs. I ran no Cargo command, application, device or SQLite reader. Regular-studio preservation was checked through the recorded snapshots; I did not open its catalog or retained objects. This audit changes only this report.

## Fingerprints and source stability

The final [packet](../verification/rust-native-catalog-vm-tests.json) is **66,095 bytes**, SHA-256 `45641912e5fe9510a6db343d1a2e77075feda3dfcc44fea69ce30d97c1955a23`. Its [corrected gate](../../.runtime/rust-catalog-vm/gate-corrected/gate-run.json) is **104,281 bytes**, SHA-256 `75418c134a8c43b05b1f87470ff7c0daf9a81c4a11c328cd3d8f91fa37d00157`.

I verified **810 fingerprint appearances representing 282 unique path/hash/size records**, including qualified source, raw logs, contracts, reviews, focused receipts and retained diagnostic artifacts: zero mismatches. All **258** packet source records equal the final gate's before and after records and the current files. No crate Rust/JSON/TOML input was omitted from that inventory. All nine packet command records equal the actual gate records and report exit zero. The runner records output and before/after fingerprints, checks test selection, and explicitly supplies the serial harness. It was inspected, not executed by this reviewer.

The bound [scope contract](../contracts/NATIVE-CATALOG-VM-SCOPE.md) is 6,166 bytes, SHA-256 `6e428836996bf9c016249ddb1ce3c8fd7d01ff50030eac7d110fb5a1ad0f56af`. The unchanged [source review](RUST-CATALOG-VM-REVIEW.md) is 7,726 bytes, SHA-256 `f4682ffbc9a625bc316512c9883c83892d944f6658d8f9d537459d3f885fe16e`. That review accurately leaves the then-running final gate pending; this audit supplies its later evidence.

## Actual executions

I recomputed result totals from the raw test summaries and matched the named new cases to source and successful raw test rows.

| Actual final command | Passed | Failed | Ignored | Result groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,303 | 0 | 9 | 72 |
| Optimized recording budget | 9 | 0 | 0 | 1 |
| Optimized engine projection | 35 | 0 | 0 | 1 |
| Optimized store library | 165 | 0 | 1 | 1 |
| Optimized session library | 120 | 0 | 2 | 1 |
| Optimized store schema regression | 12 | 0 | 0 | 1 |

The optimized total is **341 passed**, not an inferred count from command names. Formatting, workspace/all-target/all-feature Clippy with warnings denied, and the default-feature all-target workspace build also succeeded. All **20 new case names** occur exactly once as successful tests in the workspace log and once in their corresponding optimized suite: four budget cases, fifteen catalog-scope cases and one public-publisher case. Fourteen scope cases exercise real isolated SQL boundaries; the remaining primary-code matrix constructs errors to test classification and is not physical I/O-failure evidence.

Every final test command explicitly uses `--test-threads=1`. Embedded writer races, retirement/publication races, append-group process interruption and SQLite ownership-lock tests still executed successfully. This qualifies the serial harness with those embedded concurrency cases, not parallel-harness stability. Ignored helper rows were not counted as successful tests.

The public-publisher oracle uses an authenticated synthetic performance and a 500,000-step ceiling. A heavy operations trigger exhausts that ceiling after candidate/history/receipt writes become visible; the test checks exact typed interruption, complete schema/table-cell rollback and unchanged originals. A fresh budget succeeds on the same store. With originals parked, exact recorded-receipt replay succeeds for an already-cancelled caller without additional caller usage or catalog changes. This is device-free execution, not native performance capture.

The [first failed gate](../../.runtime/rust-catalog-vm/gate-first/gate-run.json) remains retained: formatting passed, then Clippy exited 101 for `collapsible_if` in cleanup. Its artifact fingerprints match. The nested condition was collapsed without changing cleanup order; the complete corrected gate subsequently passed. Historical source snapshots were not falsely compared against current source. Earlier publication diagnostics remain historical; no weakened ownership guard or claimed parallel fix follows from this gate.

## Preservation and qualification

The [before snapshot](../../.runtime/rust-seal-native/catalog-vm-before-tests.json), SHA-256 `b9e009ebadfe4d6dcd1a7ec8d9669e5b9524fc04c6c16deb6a1ca7dedf0dc9d1`, and [after snapshot](../../.runtime/rust-seal-native/catalog-vm-after-tests.json), SHA-256 `e414c2078776496bed51780f439abaa53646844e281509e36c14c3d41950d67c`, are each 51,310 bytes. Apart from timestamps, they match exactly: **13 tables, 403 rows, 22 exact project IDs and 80 protected file records**, including table columns and row hashes. Both record successful integrity checks.

Both snapshots bind the unchanged 8,613,888-byte catalog to SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. This carries forward the schema-6 baseline through unchanged bytes; the snapshots do not independently record a fresh `user_version` query. I also hashed the actual installed 23,062,544-byte executable: SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`, matching both snapshots and the packet. No replacement package is claimed.

Current README/status/plan/release wording distinguishes this scoped software gate from native acceptance. Release row H05's stale next-work sentence was corrected and read back. The new contract selectively supersedes the earlier publication contract's between-statement limitation; the earlier packet remains historical evidence.

Statement/trigger VM interruption, same-budget source work, detach-before-cleanup, reusable-connection retirement and successful-commit preservation are bounded qualifications. Busy waits, filesystem/native stalls, fixed transaction-control fences and panic cleanup failures do not acquire elapsed-time guarantees. Legacy paths remain outside this scope. Source-aware archive closure and guarded reader/list/attachment consumers, durable exact-request admission, native controls and responsiveness still require integration and acceptance. Physical disconnect/power loss, native input/output, listening, long-performance capacity, MIDI 2/MPE, AI/plugin integration, Windows and the full goal remain unqualified by this packet.
