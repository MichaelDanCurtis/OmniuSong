# Projection archive preparation: independent evidence audit

Date: 2026-10-22. **Decision: bounded acceptance; passed: true.** No missing or mismatched qualified reference, current source fingerprint, execution total or preservation comparison remains. This accepts archive preparation and ordinary-archive compatibility. It does not accept complete source-semantic projection archives or native admission.

I independently read/hash-checked the actual packet, sources, contracts, reviews, raw logs and snapshot JSON. My commands used read-only file inspection and Python standard-library JSON, SHA-256 and regular-expression checks. I ran no Cargo, tests, formatting, build, SQL, application or device operation. I did not open or hash the actual default catalog or its protected objects; preservation was compared through the bound snapshots. Only this audit report was written.

## Bound evidence and source stability

The final [packet](../verification/rust-native-projection-archive-prep-tests.json) is **68,075 bytes**, SHA-256 `78c2329b9879db260ee69db0e3ff66a90c499527a0e482400461e82c13be13b1`. The [final gate receipt](../../.runtime/rust-archive-prep/gate-final/gate-run.json) is **105,371 bytes**, SHA-256 `985f59157810e4383110e6a96a9fb3b717d1eb5bb020a649ce66a3f3225ace15`.

I checked **823 fingerprint appearances / 289 unique path/hash/size records**, including final source snapshots and logs, focused logs, historical failed-gate artifacts, the contract, design and source review: zero mismatches. All **261** qualified current inputs equal both final before/after source inventories and the packet inventory. No crate Rust/JSON/TOML input was omitted. Historical failed-gate artifact hashes were checked without incorrectly requiring their old internal source snapshots to equal today's corrected files.

All nine final command records exactly match the gate receipt and have exit zero. The runner explicitly appends the serial test argument, retains raw output/exit status, stops on failure or empty test selection, and records before/after source hashes. Its successful-group-only summary limitation on failed logs is disclosed below; I recomputed totals from both `ok` and `FAILED` result lines rather than trusting that summary.

The [preparation contract](../contracts/NATIVE-PROJECTION-ARCHIVE-PREP.md) is bound at SHA-256 `7ae2352484d5cdb2a767efdfbea9bea03269d0ca13c4ac17ed4ff2ccd95a69a2`; the [design](RUST-PROJECTION-ARCHIVE-DESIGN.md) at `cfe31ffe1bea508c464e94b2270b1df66979868477f9b3d3cbb177917ede5fa4`; and the unchanged [source review](RUST-PROJECTION-ARCHIVE-PREP-REVIEW.md) at `565cd5deb1319eac61e776c3346173c84e137a1271b25d342b0a401f562922d4`. The review's pending-gate statement correctly describes its earlier freeze; this audit supplies the later final evidence.

## Actual final results

| Raw execution | Passed | Failed | Ignored | Groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,318 | 0 | 9 | 72 |
| Optimized store library | 180 | 0 | 1 | 1 |
| Optimized engine projection | 35 | 0 | 0 | 1 |
| Optimized store schema regression | 12 | 0 | 0 | 1 |
| Optimized archive review | 8 | 0 | 0 | 1 |
| Optimized archive-copy review | 10 | 0 | 0 | 1 |

The optimized total is **245 passed**. Formatting, warnings-denied workspace/all-target/all-feature Clippy and the default-feature all-target workspace build also succeeded. The actual [workspace log](../../.runtime/rust-archive-prep/gate-final/gate-workspace.log) is 153,857 bytes, SHA-256 `88023b34feabc123b1d0c6eb65082667cc015ee3a1cb469bb3ea02fed9d2e5d8`.

All **15 new cases** exist as source tests and occur exactly once as successful rows in both workspace and optimized-store logs: six collector tests, eight operation tests including the isolated SQL collision matrix, and one public ordinary-project roundtrip. I checked the named rows rather than inferring execution from filters. The public regression preserves flattened projection-looking edit extensions through export, identity restore, copy, mapped-edit replay, reexport and identity restoration of the copy. It is ordinary-archive compatibility evidence, not a projection archive roundtrip.

All final test commands use `--test-threads=1`. Embedded copy-admission races, source-action writer races, retirement/publication races and append-group process interruption still ran successfully. This does not qualify the parallel harness. Ignored helper rows were not counted as passes.

## Retained diagnostics

The [first gate](../../.runtime/rust-archive-prep/gate-first/gate-run.json) stopped at Clippy exit 101 for three unnecessary one-element test-slice clones. Its raw diagnostic matches the recorded correction to `slice::from_ref`.

The [intermediate workspace log](../../.runtime/rust-archive-prep/gate-corrected/gate-workspace.log) terminates at **179 passed / 1 failed / 1 ignored** in the store library. The runner's earlier **1,027 / 0 / 8** successful-group aggregate was partial, not acceptance. The failing established test required `Unknown archived operation wrapper`; the parser decoded an unknown wrapper's payload too early. The retained source correction restores the exact envelope precheck before typed payload decoding. That previously failing test appears as successful in the final workspace run. Earlier focused 5/8/9/22 counts remain historical focused evidence, not substitutes for the final gate or 22 new cases.

## Preservation and delivery limits

The 51,310-byte [before snapshot](../../.runtime/rust-seal-native/archive-prep-before-tests.json), SHA-256 `9293f7bdba636c3d10be6de2f0eb8f102a3577cd4e1df6ff47af932b2da62e4f`, and equally sized [after snapshot](../../.runtime/rust-seal-native/archive-prep-after-tests.json), SHA-256 `98f8eae13aa1fff367bcd6c8c5061cc43b0296eeb2b5fb6dd4af12c428f8a5c3`, differ only in timestamp. All table columns/row hashes, **13 tables / 403 rows / 22 exact project IDs / 80 protected file records**, catalog and binary records match. Both record successful integrity checks.

The unchanged catalog record is 8,613,888 bytes, SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. Unchanged whole bytes carry forward the schema-6 baseline; these snapshots do not independently record a fresh `user_version` query. I separately hashed the installed 23,062,544-byte executable and matched SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. No new package or native execution is claimed.

Actual source readback confirms the explicit projection-request refusal and unchanged `Bundle`, `Proof`, metadata/reference, list and unmigrated attachment admission guards. Current README/status/plan/release claims correctly describe preparation, preserved ordinary behavior and closed nonempty projection archive admission. Full snapshot traversal, binding memo, typed initial-publication linkage, private-archive SQL error isolation, expected postimage comparisons and typed completion/cleanup outcomes remain unimplemented by this slice. Final-path source authentication after installation remains a selected later design with disclosed rollback orphans, not delivered relocation authority.

Complete OSAR export/identity-restore/copy semantics, separate schema-3 inert OSRE regression, guarded consumer integration, durable native admission and named-binary review/edit/history/archive/restart/audition remain required. This packet establishes no native input/output, listening, sustained-capacity, inference, MIDI 2/MPE transport, Windows or full-goal acceptance.
