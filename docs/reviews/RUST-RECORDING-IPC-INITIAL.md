# Independent recording, IPC and edit-ledger review

October 20, 2026, 18:03 UTC, Darwin arm64. The completed gate reports **96 passed, 0 failed or ignored** across the native workspace, including **29 new independent regressions**. Formatting and Clippy with warnings denied also pass. This is a bounded storage/transport qualification, not completion of live recording or the full native migration.

## Findings and dispositions

| Finding | Correction reviewed | Behavioral disposition |
| --- | --- | --- |
| A hard-link alias could use a different sidecar lock to interrupt a live journal owner | Unix acquisition rejects databases with `nlink != 1` before opening SQLite, after locking and before interruption; canonical symlinks share the existing owner | Independent test passes: hard-link paths reject; the original owner stays Recording, retains its bytes and can append. Ordinary/symlink second owners also reject. Windows alias qualification remains open |
| Recovered sealing checked count but did not bind the selected bytes to the original interruption index | An immutable `recovery_index` is captured in the interruption transaction. Its full root is checked against the retained receipt, then its selected original prefix is compared with current chunk hashes | Independent full-length and shortened-prefix replacements with recomputed per-chunk digests both reject without sealing or deleting originals |
| Derived `Debug` on `WorkerHello` exposed the launch nonce to callers formatting it | Explicit Debug redacts the nonce; the coordinator added direct and nested-envelope checks | Core handshake regression passes. Launcher-owned bootstrap/state do not expose the nonce through Debug; arguments/environment/errors do not carry it. Arbitrary strings deliberately supplied by a peer are not secret-redaction guarantees |

No additional major correctness defect remains identified within the reviewed implemented boundaries. Checksums, immutable SQL triggers and retained roots detect accidental corruption and inconsistent partial rewrites; they are not authentication against an actor replacing every database root/receipt or the entire file. External replacement/renaming of an active journal is unsupported.

## Fresh evidence and independent coverage

The coordinator executed `cargo test --workspace --all-features --locked`; the complete log is `.runtime/rust-migration/recording-ipc-workspace-tests.log`. Nonzero suites total:

| Component | Passing tests |
| --- | ---: |
| App | 10 |
| Core/contracts | 15 |
| IPC framing, process fixtures and independent regressions | 6 + 6 + 9 |
| Recording implementation and independent regressions | 16 + 10 |
| Store implementation, operation review and prior independent regressions | 8 + 10 + 6 |
| **Total** | **96** |

The coordinator's completed final process exited 0; this review independently read the resulting logs and migration receipt:

| Check | Result | Evidence under `.runtime/rust-migration/` |
| --- | --- | --- |
| `cargo fmt --all -- --check` | Exit 0 | `recording-ipc-format.log` |
| `cargo test --workspace --all-features --locked` | Exit 0; 96 passed | `recording-ipc-workspace-tests.log` |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | Exit 0 | `recording-ipc-clippy.log` |
| `cargo run --locked -p omniusong-store --example qualify_legacy -- data .runtime/rust-migration/qualified-recording-ipc docs/verification/rust-native-migration-stage2.json` | Exit 0; 19 separate native imports reopened, 30 original project/audio files unchanged | `recording-ipc-migration.log`; `docs/verification/rust-native-migration-stage2.json` |

The migration receipt records exact original JSON retention and musical/unknown-field equality for every project. Qualification copies use a separate native directory; their success does not establish a take-reference musical schema.

The three new independent integration files are:

- `crates/omniusong-store/tests/operation_review.rs` — ten tests. Same-ID identical requests return the original committed snapshot after later edits without applying it again; conflicting content, unsupported intents and stale revisions reject. Undo/redo receipts survive redo-branch truncation. Concurrent identical requests publish one edit. A failure at the final operation insertion rolls back project, history/cursor and receipt together. Additive schema-1-to-2 migration retains project/history/asset rows and exact import receipt bytes; a future-version fixture rejects without changing its bytes. Just-over-cap project/history/request/result/import/blob rows fail through the public APIs without publishing edits. Unknown huge integers/long decimals remain exact.
- `crates/omniusong-recording/tests/review_regressions.rs` — ten tests. Caller-buffer mutation cannot change committed originals. Literal raw-envelope JSON bytes, source/generation, clocks/anchors, opaque provenance precision and interpretation-bound hashes survive sealing and fresh reopen. Skipped/wrapped sequences, discontinuous ranges, wrong versions/clocks/interpretations, conflicting retries, fabricated gap bytes and malformed event records reject. Bad tails remain retained while an explicit recovered manifest references only its proven original prefix. Sealed payload/header/manifest tampering rejects on fresh open. Injected errors test all six boundaries; a separate test launches and kills owned helper processes at all six transaction boundaries, then freshly opens the database. Only committed chunks/manifests survive; unfinished capture becomes Interrupted and never resumes.
- `crates/omniusong-ipc/tests/review_regressions.rs` — nine tests. An oversized announced control frame rejects after the four-byte header without reading a payload. Clean EOF differs from truncated header/body. Malformed/version/channel/replay failure permanently poisons admission without resynchronizing. Local rejection emits no bytes or sequence advance; partial writes poison retries. Rejected admission is non-mutating, later gaps are permitted, and wrap rejects. Fragmented reads retain exact unfamiliar numbers. Real socketpair peer closure preserves the preceding complete frame boundary. Actual subprocess fixtures with a correct private challenge/role/PID but wrong WorkerId or stale SessionId reject.

The two recording fault tests require the `test-hooks` feature; the passing gate used `--all-features`. Helpers announce the selected boundary, flush the marker, and only then are killed/reaped by their owning parent. They do not signal any app, device, or unrelated process. All disk fixtures are synthetic, UUID-isolated under ignored `.runtime`; the source 19 legacy projects/11 assets are outside these test paths.

Complementary owner tests passed: streamed audio above the legacy 30 MiB cap, MIDI-only capture above 4,096 embedded events, full-u64 host ticks, signed SQL boundaries, unrelated main-database write contention, real worker echo/fresh launch IDs, wrong nonce/role/claimed PID, plain-stdio rejection, negotiated ceilings and absolute silent/slow-peer deadlines. Generic framing uses caller-supplied blocking I/O; the Unix wrapper supplies bounded absolute deadlines. Source review confirms byte limits before bounded row materialization and typed/channel ceilings before incoming frame allocation.

## Publication and acceptance limits

The journal independently commits immutable chunks and a sealed manifest. Main-store publication of a sealed take remains **pending**: musical `ProjectDocument` still accepts schema 1, and ordinary asset resolution uses the schema-1 whole-file bridge limits. An opaque metadata string containing a take ID/hash is not a validated musical reference. An explicit versioned native reference/resolver must verify the manifest and immutable objects before the later project/history/operation transaction. SQLite store schema 2 adds the edit ledger; it does not provide that musical schema migration.

The implemented ledger supports Rename, SetTempo, Undo and Redo. Prepared edits and candidate acceptance explicitly reject. The draft UI still uses its existing atomic save; stable UI operation IDs, IPC-to-store integration and candidate acceptance are separate work. An old replay receipt is historical evidence; a caller must load the latest project before replacing a newer UI view.

Unix worker identity is bound to a private inherited socketpair, fresh challenge and supervisor-owned Child handle. It does not attest kernel peer PID per message, sandbox a worker, close every unrelated inherited descriptor, authorize every typed operation or supervise descendants. Windows private IPC explicitly returns unsupported.

Live device capture, callback/transfer queues, 250 ms audio/50 ms event aggregation, performance alignment, engine graph activation, audible output, actual power-loss/fsync/disk-full behavior, archive consistency, Windows ownership/IPC and the thirty-minute stability gate are unqualified. Original recordings are preserved as immutable data; this stage neither publishes them into a project nor establishes a complete recording workflow.

## Reviewed source snapshot

SHA-256 read after the final source freeze, October 20, 2026, 18:03 UTC. The current protocol document explicitly distinguishes the adopted production topology from this implementation's launch/ownership limits.

```text
2eeebccdece52ffe9947908c78258298328dbb2c37b1e1d3305159c1085a48af  crates/omniusong-core/src/contracts.rs
d67506c80ced4fa847ec4d9f2f65d5233de794f1acf837e7ba596bc888f2c40b  crates/omniusong-store/src/lib.rs
6c0fba45bd509e2428ce24b9e106cd43a8d9c55ed7ae016e5d96a4ed4c754913  crates/omniusong-store/src/operations.rs
245844942664c18c60e970be89df34a2b68e8b41700b118adcf769ce83621920  crates/omniusong-recording/src/journal.rs
25867a282d978aeb61dcc86d6b9bc7a694d5af8bc8f52c8e48fd8fcb1e4166fa  crates/omniusong-recording/src/types.rs
cd8b4fb53dc68fbb24d9a8b9c817d2967cd8981733669aa3f4e7a958a754c498  crates/omniusong-ipc/src/lib.rs
1944638a1590569fd4e9cb4fa3e78437a824f5e1b17633cbb4c489c765d11edc  crates/omniusong-ipc/src/unix.rs
bccac2a3d1bf4f57a4d5ff8bb250ec9b3cb26cf154d9e479ab5697087bb23a84  docs/contracts/NATIVE-PROTOCOLS.md
f13b9fcb8aa599fadcfd71a67544cdb5dba4fb379e9ed44e7a3608642e20f6ae  docs/contracts/NATIVE-RECORDING.md
1105ef20fdb092bfa8c487e95d200d4571bc4383e694c97636ea6349965e3ce7  docs/contracts/NATIVE-IPC.md
99d4dbf18b04564d434aa8a7e37079bc83a56176f5964ae22ae712a1b263e661  docs/contracts/NATIVE-EDITS.md
1e10bd0ac0d06327237f4a94ac3d3e529270a7b8dc1f5270bb61621478d8ee65  crates/omniusong-store/tests/operation_review.rs
ccb942295147cceb5162dfd897154592549f470c8c66f5662d55a20b3544673a  crates/omniusong-recording/tests/review_regressions.rs
3fae358ea71efa1d464b14fe73a3888fdb9a8592ba38c6db029c86751322d127  crates/omniusong-ipc/tests/review_regressions.rs
```
