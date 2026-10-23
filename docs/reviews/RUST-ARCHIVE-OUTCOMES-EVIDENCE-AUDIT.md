# Ordinary archive outcomes — evidence audit

Date: 2026-10-22. **Decision: bounded evidence acceptance, with the audit-process deviation disclosed below.** I found no mismatched qualified artifact, current source fingerprint, command total, new-case execution or preservation comparison in the frozen final packet. This accepts the recorded software evidence for ordinary OSAR outcome/error handling. It does not activate projection archives or qualify a native workflow.

## Method and audit boundary

This is an evidence audit, separate from the [independent source review](RUST-ARCHIVE-OUTCOMES-REVIEW.md). I inspected the runner and packet-builder source, parsed JSON and logs, compared test declarations against baseline commit `c5ea82849b9e7386c7059f8a6e2182ba7f4b81c7`, and hashed recorded artifacts. I ran no Cargo command, formatter, build, test, SQL, SQLite API, application, audio or device operation. Only this audit document was written.

**Boundary deviation:** my first generic fingerprint walk mistakenly treated the packet's repository-relative `preservation.catalogFile` and `preservation.installedBinary` records as ordinary artifact links. It consequently read the protected live catalog and installed executable as raw bytes for SHA-256/length comparison, contrary to the explicit instruction not to open them. Both matched their captured records. No SQLite/API operation, executable launch or protected-file write occurred. I reported this immediately, made no repeat reads, and corrected the allowlist. The corrected artifact-only walk below excludes those two literal live-payload records; its counts do not include the mistaken reads. The source review's statement that its reviewer did not open protected paths applies to that separate reviewer, not to this audit.

## Packet, artifacts and current source

The final [packet](../verification/rust-native-archive-outcomes-tests.json) is **71,383 bytes**, SHA-256 `b2cb9982aec51692164fb4e318bace3b6f5b3e0fdf3be84fa33664f9b1c5900c`. Its [corrected gate record](../../.runtime/rust-archive-outcomes/gate-corrected/gate-run.json) is **108,550 bytes**, SHA-256 `5003cf07f7cd5bce139b0f6c59c7f8144fb5d8728b5387793a4f385d4505f9b4`.

The corrected allowlist walk verified every nested ordinary fingerprint record in the packet: **293 records naming 292 distinct files**, with no missing file, length mismatch or SHA mismatch. These include qualified source inputs, command logs, runners, the builder, both failed-gate records/logs, preservation snapshot artifacts, the contract, source review and preceding closure packet. The two live preservation records were treated as captured data rather than followed again.

All **269 source inputs** match current ordinary files. The gate's before/after source manifests are identical, and the packet contains exactly that manifest, without duplicate paths. I independently reconstructed the runner's required set from the preceding packet, all crate `.rs`/`.json`/`.toml` files and the new contract; no required path is missing or extra. This is the recorded input set, not a claim that every unrelated workspace file was qualified. The coordinator's later status-document edits are outside this set.

The [contract](../contracts/NATIVE-ARCHIVE-OUTCOMES.md) matches **6,474 bytes**, SHA-256 `b9589c4b9030a0c36d02bb9b0d186b13d3c1a567673210abf2c9acf6f5382ace`. The final [source review](RUST-ARCHIVE-OUTCOMES-REVIEW.md), including its engine-assertion addendum, matches **14,233 bytes**, SHA-256 `dad0200ab5cd058c88bc5040e7107d47ec792ea37f31d833b1480366c4a2a4fb`. Its then-pending corrected gate is resolved by this audit; the historical review was not rewritten to claim execution.

## Commands and named cases

All nine recorded command vectors match the [runner](../../.runtime/rust-archive-outcomes/run-gate.py), and the packet's command records exactly match the corrected gate. Every exit code is zero. Formatting, all-target/all-feature warnings-denied Clippy and the workspace all-target build passed. All recorded test commands explicitly used `--test-threads=1`; parallel-harness qualification is not claimed.

| Recorded test command | Passed | Failed | Ignored | Result groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,362 | 0 | 9 | 72 |
| Release store library | 206 | 0 | 1 | 1 |
| Release session library, all features | 123 | 0 | 2 | 1 |
| Release engine projection review, all features | 50 | 0 | 0 | 1 |
| Release archive review | 8 | 0 | 0 | 1 |
| Release archive-copy review | 10 | 0 | 0 | 1 |

I independently parsed both `test result: ok` and `test result: FAILED` summaries and checked named failed rows. The qualified totals match the packet, with no failed group or failed named row. The selected release total is **397 passed**, with three ignored helpers; it is separate from the workspace total.

I compared actual test declarations against the baseline and the packet's complete name list. All **22 added cases** appear exactly once as passing in the workspace log and exactly once in their assigned release log: seven archive-outcome, four boundary, five copy/inspection and three catalog-scope cases in release store, plus two session-library and one evidence-job diagnostic case in release session. The corrected existing engine test `fresh_generic_paths_cannot_claim_projection_namespace_but_exact_legacy_receipts_still_replay` also appears exactly once as passing in workspace and release engine. This checks execution of the final assertion rather than relying on its earlier failing run.

## Failed and preliminary records

The [first gate](../../.runtime/rust-archive-outcomes/gate-first/gate-run.json) remains **failed**: formatting passed, then Clippy exited 101 because all test-event variants shared `Before`. No workspace qualification follows from that attempt.

The misleadingly named earlier [gate-final](../../.runtime/rust-archive-outcomes/gate-final/gate-run.json) also remains **failed**. Its workspace partial aggregate is **646 passed, 1 failed, 4 ignored, 17 groups**, exit 101. The failed engine result group is **49 passed, 1 failed**, and the failed-result regex includes it. Its old oracle expected a bare `Invalid`; the recorded result retained the same refusal inside typed archive context before acquisition. The corrected oracle additionally checks `NotStarted`, absent staging/observed hash and the requested pin. The corrected complete gate, rather than either failed record, supplies qualification.

Both failed records' command/log fingerprints and recorded aggregates match their artifacts. Their `sourceStable: true` fields mean only that their own inputs did not change during those failed attempts. The packet also explicitly labels early focused/compile observations as transcript summaries without standalone raw logs. Those summaries, including the 108/1/1 focused result and neighboring assertion correction, are historical diagnostics rather than independently bound qualifying runs.

## Preservation evidence

The **51,310-byte** [before snapshot](../../.runtime/rust-seal-native/archive-outcomes-before-tests.json) matches SHA-256 `7d6441eafa1c1ac691a79a898899ae847c91279e29dfed2b84009cceda0e2694`; the equally sized [after snapshot](../../.runtime/rust-seal-native/archive-outcomes-after-tests.json) matches `86dc3bc5ba90dbbda83e37c7d16555d8e0d5b05669d75d7d56f6cfe0e888b8c8`. Their contents differ only in timestamp. All table columns/row records, exact project identities, protected-file records, catalog record, installed-binary record and successful integrity result match: **13 tables, 22 projects, 403 catalog rows and 80 files**.

I also verified the preceding closure packet's bound after-snapshot artifact and compared its captured records with this before snapshot; all five protected record groups match. The catalog record remains **8,613,888 bytes**, SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. Its whole-byte identity carries the established physical schema-6 baseline. The installed executable record remains **23,062,544 bytes**, SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. These are the captured preservation comparisons; the two unintended raw reads described above add no SQLite integrity or application acceptance claim.

## Accepted boundary and remaining work

The evidence supports actual ordinary export, identity restore, copy, replay and inspection outcomes; separate durable/committed completion and private cleanup warnings; per-handle SQL provenance; explicit live rollback/error retention; and session preservation of retirement and uncertainty facts. Boundary hooks and denied transaction controls are synthetic fault oracles, not physical disk failure or power-loss simulation.

The packet and contract consistently keep nonempty projection archive admission closed. This gate does not qualify cumulative archive governors/VM limits, exact expected destination postimages, final installed-path performance authentication, a new crash-reconciliation guarantee, native successful-warning presentation, a new installed package, Windows, device input/output, playback/listening or complete milestone delivery. Successful compatibility replies still discard cleanup warnings by design. The governed coherent projection-route integration and the separate native workflow gates remain open.
