# Retained projection validator: independent evidence audit

Date: 2026-10-22. **Decision: bounded acceptance.** I found no missing or mismatched qualified reference, source fingerprint, command total, new-case execution or preservation comparison in the final packet. This accepts the source/software evidence for the read-only retained-closure diagnostic. It does not activate projection archives or qualify a new native binary/workflow.

The audited [packet](../verification/rust-native-retained-projection-closure-tests.json) is **70,135 bytes**, SHA-256 `3e95e1353956b37d2b0355e24febf86a30925b39402130ba06411b1cf95a4762`. Its [gate record](../../.runtime/rust-archive-validator/gate-first/gate-run.json) is 107,019 bytes, SHA-256 `a97c9e57ee5eedf6e1ad1fbdb56d0c77308e19be0d319ba8016106e7efa6fd3e`.

## Files and source stability

I read and independently SHA-256 checked the packet's and gate record's nested fingerprint records. Across **827 fingerprint appearances**, **826 were directly checked**, representing **285 distinct source/document/log/artifact files**; all matched both hash and byte length. The single installed-binary reference was compared with the preserved before/after snapshot records rather than opening the protected binary. Protected catalog/object records inside those snapshots were likewise compared as artifacts, not reopened.

The packet's 265 source inputs, gate-before inputs and gate-after inputs are identical, and every one of those **265 current files** matches its recorded length and hash. The gate runner is itself bound and checks command exit status, nonzero test selection, failed test totals and before/after source equality. No historical source snapshot was mistaken for a requirement that an older source revision equal today's files.

The current [contract](../contracts/NATIVE-RETAINED-PROJECTION-CLOSURE.md) matches its 5,566-byte fingerprint, SHA-256 `5d0eb081d24cf7fa14563722683506f43881cc2876511d3ef7f1253dfdf11724`. The [design](RUST-PROJECTION-ARCHIVE-DESIGN.md) matches 20,370 bytes, SHA-256 `560fb3ef09d715f04ca3621a8d4f82d3f553d28545a6a5c8f3d5d6acb2bfae34`. The frozen [independent source review](RUST-ARCHIVE-PROJECTION-VALIDATOR-REVIEW.md) matches **12,961 bytes**, SHA-256 `697820c604e0a57c6e59e1ff0a4df3e5d0f5daca333e2b5166e42a053bb9e6c9`. Its then-pending full gate is resolved by this separate audit; the historical review was not rewritten.

## Actual commands and tests

The packet and gate record contain the same **nine commands**, all with exit code zero. Raw formatting, warnings-denied offline workspace/all-target/all-feature Clippy and default offline workspace/all-target build logs agree with those records. I independently parsed each raw test-result group rather than adopting the packet's aggregate.

| Actual test command | Passed | Failed | Ignored | Result groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,340 | 0 | 9 | 72 |
| Optimized store library | 187 | 0 | 1 | 1 |
| Optimized engine projection review | 50 | 0 | 0 | 1 |
| Optimized store schema review | 12 | 0 | 0 | 1 |
| Optimized archive review | 8 | 0 | 0 | 1 |
| Optimized archive-copy review | 10 | 0 | 0 | 1 |

The five actual `--release` commands total **267 passes**. Every test command explicitly uses `--test-threads=1`; this qualifies the serial harness. Raw workspace rows still show embedded process/concurrency cases executing, including spawned typed-worker exchange, concurrent copy admission, concurrent source retirement/publication and simultaneous first-open tests. This does not establish parallel-harness stability.

I compared the actual `#[test]` names in the two new files with the packet's list. All **22 new cases—seven store metadata/resource cases and 15 real-original engine cases—appear exactly once as passing in the workspace log and exactly once in their assigned optimized log**. The full gate therefore qualifies the final tuple-grouped mapping helper as well as the corrected semantic tests; it does not rely solely on the earlier focused runs.

The real-original cases use device-free recovered v1/v2 performance journals and production ingestion/publication paths. They cover edits/deletion, hidden history, ordinary audio/performance role separation, reused originals, coherent false claims, exact binding reuse, source absence/replacement and same-store reuse after sticky quota/cancel refusal. The final operation-only false-music and initial-mute cases actually prune the old publication history edge, first accept their valid controls, and then exercise the intended retained-operation check. The mute case requires the precise initial-lane error. Full isolated catalog-cell/schema/file comparisons surround the diagnostic calls.

The two executed archive-copy generations are **ordinary projects copied before a leaf projection is published**. They qualify inherited ordinary mapping/ancestor handling by the diagnostic, not nonempty projection export or copy. Associated synthetic proof records are explicitly synthetic; their successful internal checks are not evidence of historical user confirmation or an executed copy.

## Retained diagnostics

All five focused log fingerprints match. The first store run was **5 passed/1 failed**, caused by its coherent-replacement fixture updating a proof primary key before child foreign keys. The repaired fixture uses one deferred-constraint transaction and still checks constraints at commit; the intermediate 6/0 and final 7/0 runs are distinct. The first engine run was **14 passed/1 failed** because an absent publication request surfaced as `Database(QueryReturnedNoRows)` instead of the expected semantic refusal. The narrow optional-row correction preserves actual SQLite failures, and the corrected engine log is 15/0. Neither failed run is counted as acceptance.

The packet also accurately retains the source review's fixed ancestry-work and foreign-project-identity prepayment issues and the earlier test-path masking. A 256-hash subset regression is not described as a complete maximum-sized 256-proof catalog test. Authentication once per distinct held original is supported by inspected control flow; no authentication-invocation counter was measured.

## Preservation and limits

I verified the before/after snapshot artifact hashes and compared their contents exactly. They match in all table columns and row hashes, all project identities, every protected file record, whole catalog record, installed binary record and integrity result: **13 tables, 22 project IDs, 403 rows and 80 files**. The catalog record remains **8,613,888 bytes**, SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`; exact whole-file identity carries the established physical schema-6 baseline. The installed binary record remains **23,062,544 bytes**, SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. This audit did not independently reopen the protected catalog, objects or installed executable; root's captured snapshot evidence is the preservation boundary.

Current README, STATUS, IMPLEMENTATION-PLAN and RELEASE-ACCEPTANCE descriptions agree with the new totals and bounded scope. They keep the read-only count summary separate from source/playback/archive authority, retain closed nonempty projection archive/consumer paths, and leave full milestone 2 and the workstation goal open. The implementation-order text treats the validator as delivered rather than restarting its design.

I rechecked the returned summary fields and existing `Bundle::references`, `Proof::document`, archive projection-request and guarded metadata/take/list fences. The public diagnostic returns no held source or admission permit. Complete OSAR schema/index/global-row/import/object/transfer validation, private archive SQL error isolation, typed completion/cleanup outcomes, final installed-path authentication and exact destination postimages remain pending before coherent export/identity-restore/copy activation. The separate schema-3 inert OSRE regression, remaining reader/list/attachment integration, durable native admission and exact-binary workflow are still required.

My commands were read-only `rg`, `sed`, `cat`, filesystem listing, SHA-256 hashing and inline Python JSON/hash/log comparison. I executed no Cargo, formatting, build, test, SQL, application, device or network operation, opened no protected/default-store object, and changed only this audit. No new package, native input/output, listening, calibrated latency, inference, sustained thirty-minute capacity, physical-failure, Windows, MIDI2/MPE or full-goal completion is accepted here.
