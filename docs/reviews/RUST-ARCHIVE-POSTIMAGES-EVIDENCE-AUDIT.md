# Archive postimages evidence audit

Status: accepted for the bounded ordinary-archive backend evidence scope. No unresolved packet/count/hash inconsistency or source-scope blocker was found. This does not accept projection archives, protected artifact bytes independently, or a native musician workflow.

The audit reads only the coordinator-authorized repository inputs, archive-postimages gate records, and the two named coordinator preservation JSON records. It does not open any protected artifact path recorded inside those JSON files. No Cargo, formatting, SQL, application, device or native input/output operation is performed by this auditor. The auditor authored the initial ledger helper and its child tests; this evidence reconciliation is not a separate independent design review of that helper.

## Source and test scope

The ordinary export, identity restore and restore-as-copy routes integrate the exact-row ledger before their publication/commit fence. Restore plans are created under the live transaction, independently of destination triggers, and frozen before final object placement or live SQL mutation. Export expectations come from its held source, constants and original buffers. Metadata/payload expectations precede insertion; the existing bounded lineage copier validates and writes private output before the independent lineage expectation is derived from that same held source, then frozen. The fixed OSAR schema check remains separate from the schema-stability fingerprint. Identity restore consumes the privately acquired archive and records its observed digest; it does not acquire a caller-specified expected hash through this API.

The ledger has static layouts for all thirteen persistent catalog tables and the archive-v1/v2 tables. Schema admission rejects unknown tables/views, virtual or WITHOUT ROWID tables, generated/surplus/missing columns, unsupported declared types/defaults/primary keys, and unsupported persistent object ownership. The schema digest pins admitted trigger/index DDL, application ID and user version. Full final scans use physical rowid traversal and a separate observed key/hash index, comparing exact typed cells, key sets, row counts and schema against frozen expectations. This is SQL-row/schema evidence, not exact physical SQLite-file bytes or final external-object identity.

The test-function delta against `c531bae` is 25 additions and zero removals: thirteen in `archive_postimage/tests.rs`, eight in `archive/postimage_tests.rs`, and four in `archive_copy.rs`. These include independent cell framing, bounded substring work, missing/extra/substitute/swap rows, exact-reuse collisions, plan overlays, local action/intent rows, unsupported schema, field/key/counter bounds, lowered private-spool SQLITE_FULL, malformed private SQLite origin, optional-governor stops, both archive layouts, real-route trigger attacks, acquired-input replacement, same-store retry, and a coherently rewritten private copy plan. The numeric counter ceiling test seeds private counters rather than performing a multimillion-row stress run. Raw REAL-bit and signed-zero distinctions are proven by the pure framing oracle; the audit does not infer unsupported SQLite persistence behavior from that oracle.

## Resource and authority boundary

Actual ordinary routes pass `None` and retain legacy archive transaction behavior. Their new refusal caps do not qualify cumulative SQL VM cancellation, cumulative work governance or a hard time bound. The reusable helper's `Some` tests borrow the original governor; only the disposable spool installs its own progress hook. Source/destination progress ownership remains the caller's responsibility.

Each plaintext plan and each expected-plus-observed digest spool has its own 512 MiB main-file cap. Temporary files, journals, filesystem overhead and failed staging are outside those main-file caps. The 4,194,304 ceiling counts source observations and ledger insertions conservatively; rows may consume multiple credits. Per-field limits, 256-byte encoded keys, sixteen-column layouts, 256 schema objects and 64 KiB schema SQL remain explicit safe-refusal limits. Checked raw-byte accounting is not a new cumulative-byte allowance. Bounded substring buffers and prepaid full-cell interpretation do not promise bounds on every SQLite internal allocation or filesystem stall.

No projection admission, source adoption, installed-path authentication, native success-warning transport, new bundle, hardware/listening, Windows qualification or full milestone acceptance follows from these tests. Frozen expectations are one-shot in-process values, not durable crash-reconciliation authority.

## Final evidence reconciliation

The final [verification packet](../verification/rust-native-archive-postimages-tests.json) is 77,689 bytes, SHA-256 `e20c4e4405dc215031dfa7573428521ba22cd38fd9a17770784a25457bbcae6c`. Its `sourceSnapshots` exactly match both before and after arrays of the final `gate-qualified/gate-run.json`; all 274 current files were independently hashed and matched. Every source path was first checked as relative, without `..`, and within the coordinator's explicit first-component allowlist. The set is exactly the preceding outcome packet's 269 paths plus the new contract, plaintext-plan module, ledger module and two new test files. No source file is inferred from a generic filesystem fingerprint walk.

The final raw record is 110,452 bytes, SHA-256 `db450506c9169863e2af2ec64513de762fc7c36fb53e9526392d436b4f6a4e5d`, with `passed=true` and `sourceStable=true`. Its nine command arrays exactly match the packet and gate runner: formatting, warnings-denied offline all-target/all-feature Clippy, offline all-feature workspace tests, offline default-feature all-target workspace build, and the five selected release test commands. All test commands are serial (`--test-threads=1`). Every referenced final log's byte length and SHA-256 matched; raw `test result` lines, rather than the packet's summary alone, produced these totals:

| Command | Passed | Failed | Ignored | Summary groups |
| --- | ---: | ---: | ---: | ---: |
| Workspace, all features | 1,387 | 0 | 9 | 72 |
| Store library, release | 231 | 0 | 1 | 1 |
| Session library, release, all features | 123 | 0 | 2 | 1 |
| Engine projection review, release, all features | 50 | 0 | 0 | 1 |
| Archive review, release | 8 | 0 | 0 | 1 |
| Archive copy review, release | 10 | 0 | 0 | 1 |

Formatting, Clippy and the default build each exited zero. The five selected optimized groups sum to 422 passed, zero failed and three ignored; this is not an optimized run of the entire workspace. The workspace increase from the preceding packet's 1,362 passed is exactly the 25 new test functions against `c531bae`. Their packet names and per-file counts match the source delta. Each new name occurs exactly once with `ok` in both the final workspace log and final store-library release log; none is removed or merely filtered/ignored.

Other independently matched final references are:

| Reference | SHA-256 |
| --- | --- |
| Gate runner | `743a7d84903b9fa98caba5affcb04e06e675742947bfce0dc3f31c09df0fc02e` |
| Packet builder | `05f82e525cb550ad699a77f55214781c8dafba1d298858a64b2223cbfad43f6a` |
| Postimages contract | `18ce294da640b3f7143ac1b270b7d8df4ebea78b22af9a339e1130a5763c8a54` |
| Separate independent source review | `3a7f39603f9ccd81c4ed0bbf1399fa77de6db123ff3250362d22e245eea39282` |
| Preceding outcome packet | `b2cb9982aec51692164fb4e318bace3b6f5b3e0fdf3be84fa33664f9b1c5900c` |

The following nonqualifying records are preserved separately in the final packet and were independently checked, including each referenced log's SHA-256/byte length and raw test-summary sums:

| Run | Record SHA-256 | Result |
| --- | --- | --- |
| `gate-initial` | `be3c03f13bba874da30089a821555e7945e3e611b3bbafc18bfd0b5f5b82808b` | 274 stable inputs; fmt passed; Clippy exit 101; failed gate. |
| `gate-final` | `95f91076d9ffe03fabecca8836f4f2c04686a4c933d03e470ba2f10f2bad24c4` | All nine commands passed, but `sourceStable=false`; failed qualification. |

The second record has the same raw test totals as the final qualified run, but is not used as final qualification. Its exact before/after input difference is only `docs/contracts/NATIVE-ARCHIVE-POSTIMAGES.md`; all 274 after-input hashes matched the current files when checked. The final packet correctly preserves the failed initial gate and unstable second gate as distinct records. Preliminary transcript-only observations are labelled as such and are not substituted for the bound final logs.

## Coordinator preservation and pending native checks

The only preservation files opened were the named coordinator JSON records. The before record is 51,310 bytes, SHA-256 `a9edb085563beec86455754c153ef9bfad022113efeb16557698d021a72fceee`; the after record is 51,310 bytes, SHA-256 `4a88ca0bf910af00e7107d6993f0f2b6301195e6f86f6e8f4983367a017d52b7`. Both packet references matched. Their complete `tables`, `projectIds`, `files`, `catalogFile` and `binary` values are equal. Table row-count declarations match their row-digest list lengths, totalling thirteen tables and 403 rows; project IDs are 22 unique entries; protected file entries total 80. Both records declare successful integrity checks.

The catalog record remains 8,613,888 bytes with SHA-256 `8dae5fe2a7f1a2fae0e7687a30fbe48ad908e35f24a59877e8555e172e766e14`. The installed-binary record remains 23,062,544 bytes with SHA-256 `82ca1c38fa474938cbf21994435b592ca052d6a68d71dfe9e7f749739a9a25ca`. These two declarations also exactly match the preceding outcome packet. Schema 6 is carried by the coordinator's exact catalog-byte baseline, not a fresh schema query by this auditor.

This is acceptance of coordinator-record consistency. Actual default-catalog schema/full rows, protected objects and installed-binary bytes were not independently opened, hashed or queried, and no artifact path inside either JSON was followed. The packet's no-native-operation declaration is not an independently observed process/device trace. Native success-warning delivery, exact-binary UI workflow, physical hardware/listening, Windows and full workstation/milestone-2 acceptance remain pending separately. Nonempty projection archive routes and final projection installed-path authentication remain closed/unqualified in the source, contract and packet.
