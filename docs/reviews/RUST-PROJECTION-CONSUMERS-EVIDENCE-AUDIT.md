# Projection consumers: evidence audit

Accepted for the bounded source and isolated software-test claims in the [final packet](../verification/rust-native-projection-consumers-tests.json). No unresolved artifact, fingerprint, count or current scope discrepancy was found. This does not qualify an installed native workflow, Windows, physical audio, or the full workstation goal.

This audit reconciles coordinator-produced evidence; the auditor ran no Cargo, formatter, build, test, SQL, app or device operation. Reads and hashes were restricted to the finite authorized repository/evidence paths and exact baseline Git objects. Preservation JSON was compared in memory. None of its catalog, original, installed-binary or other protected target paths was opened, resolved or hashed.

The auditor authored the original eleven engine consumer tests in `consumer_review.rs`; the coordinator corrected fixtures and added the twelfth case. This is independent artifact reconciliation, not independent authorship of those tests. Astra performed the separate [production/contract source review](RUST-PROJECTION-CONSUMERS-REVIEW.md). That review correctly leaves final execution and preservation to the packet audited here.

## Exact bindings

The packet timestamp is `2026-10-23T05:36:40.563982+00:00`. SHA-256 bindings verified against actual allowed file bytes:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Final packet | 89,557 | `efadc2867e34257848a8eb37df993bcc8d36e9a2309861026545a10e2cd6c2fc` |
| `gate-final2/gate-run.json` | 122,241 | `1f27f6049eece686328c2cca877b5288548e1f24d4574e8f0a899767d402e0db` |
| `run-gate.py` | 4,307 | `b135982d024bf877b3d50cc28153002ab4c691d75dcd897fd1e1863fa0e5a3eb` |
| `build-packet.py` | 19,628 | `f5db99831de93f4b45985cd2a3f012d709894c2972dfdfc1f5f71230cc2e01fd` |
| [Consumer contract](../contracts/NATIVE-PROJECTION-CONSUMERS.md) | 14,143 | `56fa1754d2755e6c642c2b45e50fc71e0d31e2cf7e184008089ad2be08ac2c8c` |
| [Design](RUST-PROJECTION-CONSUMERS-DESIGN.md) | 20,953 | `fda17adde17a93b86175642fe32330387a4bdffc4f984237e67eafa462b87f96` |
| [Independent source review](RUST-PROJECTION-CONSUMERS-REVIEW.md) | 26,299 | `33c43aaee1d7270603e2093fd0fbf18762e6bf627f55394d586ecfba65950d03` |
| [Preceding session archive packet](../verification/rust-native-project-archive-session-tests.json) | 75,902 | `7ac79842dcb9277298ba27442016c5d6a6c81b75c5990b7ba71c66a3777d1e48` |
| Final finite allowlist, `audit-source-allowlist-v2.json` | 64,047 | `acba15b0204c76fa5a0564f4df2d0fecf0b69f8442cc02868a4ba8f9afca5df3` |

The runner, builder, gate directories and allowlists above are under `.runtime/rust-projection-consumers/`. All **331 unique allowlisted target files** match their exact recorded length and SHA-256. Every path was checked as lexical relative, without parent traversal, and each component was checked against symlinks before reading. The allowlist itself was separately fingerprinted.

All **296** source-input entries match the packet, both final gate captures and current bytes. Each has an allowed source-root component; the previous packet's 288 input paths are all retained. The packet's **322** fingerprint-reference occurrences match authorized bytes. Its two direct protected catalog/binary fingerprint references were intentionally checked only against coordinator JSON, not their targets.

The v2 allowlist changes only `docs/RELEASE-ACCEPTANCE.md` from the initial allowlist: 67,920 bytes, SHA-256 `6dec38f10a0980347ef7dddb77b385ecc6141e4c2d1dfc8f57cb0b29853b0ca1`. That tracking file is outside the 296 stable gate inputs. The A01/R01/R02/R05/R06 corrections distinguish eight crates, qualified governed library routes and store receipts from pending native/session guarantees. All **72** release State cells remain identical to baseline `5f849e5`. Current tracking introductions retain the final counts and limitations; older receipts remain historical. The linked [normal recording completion plan](RUST-CAPTURE-COMPLETION-PLAN.md), SHA-256 `946769226b6e81a8c9d2c59acac9c0424b438a70949cb3ad113823f9b739652b`, is explicitly advisory and unimplemented by this packet.

## Raw execution reconciliation

The runner's thirteen literal command definitions, including offline/locked options, feature selections and serial test arguments, exactly match the gate and packet commands. Every recorded final exit code is zero. Each raw log's bytes/hash matches its command fingerprint. Independently summing raw `test result` lines gives:

| Final command | Passed | Failed | Ignored | Result groups |
| --- | ---: | ---: | ---: | ---: |
| Formatting check | — | — | — | — |
| Workspace/all-target/all-feature Clippy, warnings denied | — | — | — | — |
| Workspace/all-feature tests | 1,518 | 0 | 9 | 73 |
| Default workspace/all-target build | — | — | — | — |
| Optimized store library | 299 | 0 | 1 | 1 |
| Optimized `take_review` | 14 | 0 | 0 | 1 |
| Optimized `projection_schema_review` | 12 | 0 | 0 | 1 |
| Optimized session library/all features | 154 | 0 | 2 | 1 |
| Optimized engine `performance_projection_review`/all features | 77 | 0 | 0 | 1 |
| Optimized `archive_review` | 8 | 0 | 0 | 1 |
| Optimized `archive_copy_review` | 10 | 0 | 0 | 1 |
| Optimized session `project_archive_service`/all features | 3 | 0 | 0 | 1 |
| Optimized headless `governed_archive_raw_routes` guard | 1 | 0 | 0 | 1 |

The nine selected optimized commands total **578 passed, zero failed, three ignored**. They repeat selected workspace coverage; 578 is not a count of additional unique tests. Formatting has an empty successful log; Clippy and the default build have successful completion footers rather than test results. The native-named guard is a headless Rust test, not an app walkthrough.

Exact baseline source objects at `5f849e5` were compared with current test declarations independently of the packet builder. There are 43 added names and four removed names; the four explicit from/to renames account for those removals and four additions, leaving **39 net new tests**:

| Source group | Net new |
| --- | ---: |
| Engine `governed_archive_review/consumer_review.rs` | 12 |
| Session `performance_source_actions/attachment_tests/projection_consumer_review.rs` | 3 |
| Store `archive_postimage/tests.rs` | 8 |
| Store `project_metadata_view/tests.rs` | 11 |
| Store `projection_metadata.rs` | 1 |
| Store `reader_tests.rs` | 4 |

Every one of the 39 exact packet-listed names occurs once as passed in the final workspace log and once in its selected optimized log. Every renamed replacement also passes once in both modes. No other removed test remains unaccounted for. The preceding packet records 1,479 workspace passes; **1,479 + 39 = 1,518**, with nine ignored in both gates.

The two earlier source-stable runs remain failed evidence:

| Retained run | Workspace passed / failed / ignored | Groups | Qualification |
| --- | --- | ---: | --- |
| `gate-qualified` (295 inputs) | 1,080 / 1 / 8 | 42 | Failed, exit 101; stopped before remaining commands |
| `gate-final` (296 inputs) | 1,491 / 1 / 9 | 63 | Failed, exit 101; stopped before remaining commands |

Their gate/log fingerprints and raw totals match the packet. Source stability alone does not qualify either. The packet labels the two obsolete guard expectations, fixture corrections and temporary capacity trace separately from accepted final evidence. Unsaved preliminary tool-transcript reports were not promoted to independently audited runs. The retained capacity trace is explicitly diagnostic; this audit does not independently reconstruct unsaved preliminary executions or the temporary instrumentation lifecycle.

## What the tests establish

The final sources and logs support actual device-free v1/v2 writer/recovery/authenticated-publication fixtures, metadata with originals absent, standalone source-aware reads, ordinary and performance-audio publication, exact historical replay, edits/history/archive/copy, source substitution and coherent false-music refusal, cancellation/quota with fresh object reuse, compact noncanonical SQL preimages, selected audio waveforms and complete thirteen-table trigger rollback. The added twelfth engine case uses actual coherent result-plus-matching-SHA substitution and an ignored insert; both are checked against independently frozen operation rows.

The Reader regressions actually probe external-process SQLite writer exclusion through successful open/drop and wrong-schema refusal, then availability after rollback. Catalog replacement and malformed/short-header cases retain their independent unchanged-file checks. Existing WAL/no-new-shared-memory refusal also passes. These are isolated Mac software/filesystem controls, not observations of the protected studio or Windows.

The three new recovered cases invoke actual typed `NativeBackend` commands and a real Verified recovery/projection path; they do **not** run an actual `SessionService` worker thread. The competing legal history choice is applied at a deterministic Ingested boundary, not a concurrent thread race. That fixture uses the same real original for projection and recovered attachment roles; separate ordinary incoming audio is covered elsewhere.

Incremental controls compare actual SQLite TEXT/BLOB bytes across UTF-8 and both UTF-16 encodings, prepay trailing scalar reads, stop during chunk work and reuse fresh scopes. Actual recursive SQL interruption and actual SQL-origin errors are distinct from the constructed SQLITE_FULL/close-only/dual-error matrices. Those matrices establish classification and cleanup bookkeeping, not physical read-close failure, disk exhaustion, power loss or postcommit cleanup failure.

The unchanged 2,000,000-byte native metadata/large-timeline compatibility case passes with the original ceiling. Baseline/current Cargo comparison adds only the existing pinned rusqlite `blob` feature; Cargo.lock has no change. This is not qualification of every maximum-size document, large history or catalog. Logical row/chunk work prepayment is not a pager-RAM, physical-I/O, VM-callback-per-chunk or hard-time guarantee.

## Preservation: coordinator JSON only

The exact snapshot JSON bindings are:

| Snapshot | Bytes | SHA-256 |
| --- | ---: | --- |
| Before tests | 51,310 | `3fe369425434f0f7bad7919985bd3e4835c8342dd0df605e2d319ce31216b094` |
| After tests | 51,310 | `caf1e06bd294ea29f6f97ca51eae7bf3c9ff923bde56908f6046443cdc5dd536` |

They record `2026-10-23T04:01:47.239293+00:00` and `2026-10-23T05:36:35.039257+00:00`. Direct structural comparison is exact for all five categories: **tables, projectIds, files, catalogFile and binary**. Both record successful integrity checks. Independently summing the recorded table counts gives **13 tables / 403 rows**; there are **22 project IDs and 80 protected-file entries**. Each recorded table's row-hash count agrees with its rowCount.

The coordinator-recorded catalog image remains 8,613,888 bytes, SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. The installed executable remains 23,062,544 bytes, SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. Catalog schema 6 is carried by that exact previously accepted image; the auditor did not query its schema, rerun integrity, inspect the binary or authenticate any protected original. These statements are reconciliation of coordinator records, not independent direct preservation measurements.

## Accepted boundary and remaining work

- Metadata validity and `source_authentication_required` do not supply source authority. Library paging beyond 512 remains required. Standalone Reader authentication covers the current document, not complete retained-history closure; legacy guarded loading and the explicit metadata-only snapshot retain their separate contracts.
- Selected waveform work authenticates only the requested media with existing lower limits and callback fences. No new whole-waveform governor, cancellation latency, deadline or combined-song authority follows.
- Fresh normal project publication and the recovered bridge use the caller's governed proof/plan/postimage path. Shared ledger `None` still means legacy SQL behavior without new VM/cumulative/time qualification; `Some` tests do not retroactively govern every ordinary route.
- Normal capture ingestion retains its authorized legacy export path. A retained original after later publication refusal is intentional. Normal session `AttachCapture` still has a generic document/library reply that can obscure earlier commitment after a later view failure; compact normal-session completion remains next work.
- Scratch cleanup is best effort, limited to remembered identities and an empty directory; warnings are not transported here and failed/foreign scratch may remain. Recovered held-plan admission fails closed on non-Unix. Reader admission fails closed without `HAS_MOVED`, including the pinned Windows VFS. Neither platform route is qualified by Mac test success.
- No instrumented authentication-constructor count, universal lower-layer physical-error priority, hard filesystem deadline, every-allocation bound or hostile post-fence mutation guarantee is established.
- The packet qualifies no newly installed binary or native window, durable exact-request native projection/archive admission, reviewed identity-SHA/export-revision UI pins, actual listening/input/output, sustained recording, Windows, MIDI 2/MPE, plugins, inference/choir, installers or full release/workstation completion. Existing raw native archive dispatch remains guarded. The full goal remains active.

No source, test, packet, execution log, protected artifact or tracking document was changed by this audit. Only this evidence-audit document was written.
