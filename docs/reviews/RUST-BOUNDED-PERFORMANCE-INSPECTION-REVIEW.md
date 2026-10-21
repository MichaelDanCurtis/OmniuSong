# Bounded retained-performance inspection: independent review

October 20, 2026, with coordinator evidence recorded October 21 UTC. Architecture and source review against the `dfcb5cf` baseline, aligned with [BOUNDED-PERFORMANCE-INSPECTION.md](../contracts/BOUNDED-PERFORMANCE-INSPECTION.md). The bounded inspection implementation passed the coordinator's integrated gate: 792 tests, zero failures, four intentional ignored helpers, 60 result groups; the subsequent Panic correction gate passed 801/0/4 in 61 groups. Both gates passed formatting, warnings-denied all-target/all-feature Clippy and the default all-target build. Exact artifacts and later native failures are separated below.

This read-only inspection API neither recovers nor normally seals a recording, changes the intent ledger, attaches material, nor resumes an engine. The coordinator separately replaced the historical `4981` native candidate, first with `a948`, then `ffc90e34`; their failed native paths are retained evidence, not acceptance of a completed performance workflow. No Cargo, formatter, native GUI/device action, live catalog access, or download was performed by this reviewer. This author independently reviewed bounded inspection and authored its independent tests, but also authored the separately assigned worker/Panic/diagnostic corrections; those production changes require coordinator and separate-reviewer approval. M2 and H09 remain open.

## Required boundary and present hazards

Adopt the proposed sole-service-worker route. `ScanPerformanceSources` enumerates a bounded local namespace; `InspectPerformanceSource` performs one explicitly requested, fully bounded review. UI receives immutable summaries and a correlated completion. It performs no filesystem or SQLite work. Reliable engine Stop remains independent of the busy storage worker.

The existing implementation cannot be made bounded merely by wrapping its final engine call:

- `recording/src/inspection.rs::CaptureReader::open` calls `inspect_connection`, which authenticates manifests/interruption receipts, scans chunks, hashes every original field, and hashes the whole physical file before returning. The normal sealed path can scan chunk payloads repeatedly through `verify_sealed` and its subsequent scan.
- `journal.rs::scan` turns row, load and validation failures into a retained-prefix problem. `performance_writer.rs::inspect_performance_snapshot` similarly catches per-group failures and returns an earlier checkpoint. **Cancellation and limit exhaustion must bypass both conversions as typed outer errors.** A completed review of genuine corruption may describe an earlier complete checkpoint; an unfinished review must not return an actionable selection.
- The engine re-reads verified chunks, decodes control records more than once, verifies reconstructed GUI originals and scans PCM for finiteness. Memory bounded by one group does not bound total work.
- `physical_hash` reads until EOF; a size preflight alone does not bound a growing source. Read at most the remaining permitted bytes plus one sentinel byte, reject growth, and check identity/length again.
- Rust checks between SQLite calls do not bound a single `prepare`, `step` or expression evaluation. A view, generated column, large sort or damaged structure can consume work before returning a row. Treat this explicitly rather than claiming every database operation is cancellable.

Keep the old `CaptureReader::open`, existing sealed/owned inspection and v1 hashes/codec behavior intact. New bounded entry points may share private validators, but the new service path must never fall through to an unbounded helper. Journal/schema/profile versions do not need to change for inspection.

## Adopted minimal interfaces

The frozen contract uses a single inspection governor, created per service request and used by both recording and engine phases:

```rust
InspectionLimits { /* explicit byte, row, chunk, group, record and VM limits */ }
InspectionUsage { /* checked cumulative consumption in those exact units */ }
InspectionBudget::new(limits, cancelled: Arc<AtomicBool>) -> RecordingResult<Self>

CaptureReader::open_bounded(path, &InspectionBudget) -> RecordingResult<CaptureReader>
SealedJournal::open_bounded(path, &InspectionBudget) -> RecordingResult<SealedJournal>
// Both readers: inspection_budget() -> Option<&InspectionBudget>.
// Their existing read/inspection methods automatically charge that governor.
inspect_performance_bounded(&CaptureReader) -> RecordingResult<PerformanceInspection>
inspect_sealed_performance_bounded(&SealedJournal) -> RecordingResult<PerformanceInspection>
```

`InspectionBudget` is public and non-Clone; its private Arc is shared only with the readers and SQLite hook. It retains the same counters throughout the request, so semantic work cannot reset allowances. The public methods are `checkpoint`, `usage`, `charge_work`, `visit_rows`, `visit_chunks`, `visit_records` and `visit_groups`. The engine adapter charges its own work through the reader's governor and propagates typed `RecordingError::InspectionCancelled` and `InspectionLimit { resource, limit, attempted }` without string matching. Zero limits are permitted; values above compiled ceilings reject. The first refusal is sticky. Fail before allocation or work that would cross a limit. Usage is diagnostic, not authority to resume a partial scan.

Keep corruption/source-format errors distinguishable from cancellation and limits. Source errors are wrapped at the session boundary and cannot masquerade as a fatal main-catalog `Database` error. A corrupt retained stage must leave the current session service generation usable. Every inspection command is read-only and has a known nonmutating outcome even when cancelled; that does not justify changing conservative failure handling for unrelated mutation commands.

The request identifies an exact 32-lowercase-hex performance stage, not an arbitrary path. Return a bounded summary containing stage identity, physical/descriptor/all-original hashes, state/profile, selected checkpoint/count/index, six stream prefixes, normal/recovered manifest identity when verified, Finish/capture-fault facts, unknown-tail and bounded problems. Do not return event payloads or full arbitrary descriptor provenance. Inspection is evidence only: this packet must expose no recovery or attachment command and mint no normal-seal capability.

## Frozen limits and exact units

These are inspection admission limits, independent of recording duration, waveform preparation and playback. A long or wide recording can remain valid yet exceed this first review path; report that visibly and retain it unchanged.

| Resource | Proposed ceiling and meaning |
| --- | --- |
| Physical file | 512 MiB (`512 * 1024 * 1024`) for the entire SQLite file, including freelist, unselected and corrupt tails. Check before opening SQLite and enforce during physical hashing. |
| Retained rows | 262,144 across all application tables and SQLite schema/sequence metadata. Count every retained row, including rows after the first bad chunk and derived receipt/index tables. No `verified_chunks` substitute. |
| Cumulative field/payload work | 1 GiB (`1024 * 1024 * 1024`) across all phases, including physical hashing. Charge bytes returned/hashed/decoded each time they are visited; repeated scans are not free. Canonical re-encoding and comparisons must also have a bounded input/record-work allowance. |
| Row/field visits | 4,194,304 cumulative visits, including schema/preflight, NULL and scalar fields. |
| Chunk visits | 524,288 cumulative visits. Charge every generic/manifest/interruption/semantic chunk visit, not just distinct ordinals. |
| Decoded record visits | 2,097,152 cumulative visits. Charge each decode/reconstruction visit, including repeated control passes. Tiny records cannot evade the byte budget. |
| Checkpoint/group attempts | 65,536 cumulative attempts, including malformed candidates; still retain the existing 63-member-plus-checkpoint and 8 MiB group bounds. |
| SQLite VM | 32,000,000 charged instructions. The implemented hook reserves one step at installation and charges one per progress callback. The original 1,000-step proposal was rejected because SQLite restarts larger progress intervals per prepared statement, allowing repeated short statements to evade cumulative accounting. |
| Schema | The implementation bounds names to 256 bytes, columns to 128 per table, schema SQL to the existing metadata ceiling, and schema rows through the shared physical-row/VM budgets. Reject views/virtual replacements and generated columns in expected source tables before payload queries. The earlier proposed separate 256-object ceiling was not adopted. |
| Returned service review | 64 KiB serialized maximum, measured before publication; bound each diagnostic and number of diagnostics. Limit failure never returns a truncated successful review. |
| Discovery | Inspect at most 128 directory entries plus one truncation probe, including invalid entries; report truncation and allow direct exact-ID inspection. No recursive search, sorting of an unbounded directory, or SQLite opening during discovery. |

The file and work ceilings are deliberately different. A 512 MiB file is not guaranteed to fit a 1 GiB cumulative scan allowance, particularly with current repeated sealed validation. Either reuse proven scan results or reject when the shared work allowance is exhausted. Do not silently reset it to make large fixtures pass. Physical-file usage is maximum observed file size; work/visit counters are cumulative. A privately shared reader handle must preserve both meanings.

The coordinator adopted the existing pinned rusqlite progress-hook feature. Check cancellation at its bounded instruction interval and stop at the specified VM-step ceiling; reclassify the resulting SQLite interrupt using the recorded typed stop reason. This introduces no new database format. SQLite execution is cooperatively interruptible; filesystem calls remain outside that guarantee. A deadline does not make a blocked filesystem call interruptible.

## Required order of operations

1. Check cancellation. Resolve the already-open canonical store root and exact stage path without creating anything. Reject redirecting/symlink directories, a symlink or hardlinked source, unknown stage syntax, missing owner guard and live ownership. Do not create a missing `.owner.lock` merely to inspect. The lock is held continuously until the read transaction and all semantic work are finished.
2. Before SQLite, check ordinary file metadata and the physical size ceiling. Read the bounded SQLite header and require rollback format (signature plus bytes 18 and 19 equal to 1). Reject existing `-wal`, `-shm`, and rollback `-journal` sidecars, including symlinks, rather than triggering repair or making claims about their contents. A hot journal is inspectable only through a later explicit forensic policy. None is deleted or recovered here.
3. Open READ_ONLY/NO_MUTEX with query-only, bounded cache, no mmap and a short busy policy. Set request-local SQLite cancellation/VM controls before potentially expansive schema/query work where supported. Validate journal version, expected ordinary tables and bounded metadata before ordinary payload loaders. Keep read-transaction consistency through both generic and semantic phases.
4. Preflight **all** retained row and byte sizes using bounded enumeration and `typeof`/`length(CAST(... AS BLOB))`. Bound before converting to Rust `String`/`Vec`; SQL text length counts characters, not bytes. Enumerate at most cap plus one rather than computing an unconstrained whole-table aggregate first. Account every column that the existing original-row digest authenticates, including opaque extension columns and bad tails. Reject unsupported schema explicitly; never omit opaque columns to fit the budget.
5. Authenticate descriptor/state, bounded manifest/interruption metadata and the generic selected prefix with the shared governor. Preserve its exact existing hashing and canonical-value rules. Stream all-original fields in at most 64 KiB fragments, and physical bytes in at most 64 KiB reads. Each loop checks cancellation and remaining allowance before work; each SQLite/raw decode validates sizes before allocation.
6. Run the full engine semantic scan through the same held reader and counters. Check between chunks, length-prefixed records, callback/control records, reconstructed events and groups. Preserve v1/v2 profile, envelope clock/source bindings, full batch versus applied-original comparison, PCM finiteness, checkpoint/member chain, Finish and fault rules. No guessed clock, resampling, event reconstruction or partial successful hash is introduced.
7. Check cancellation again before publishing success. Validate returned DTO size and exact identity. Release the read transaction and owner on every error path. Only a fully completed required review gets a usable result; the service must not publish an intermediate checkpoint from a cancelled or over-limit attempt.

The implemented physical hash reads no more than its opened file's initial length, checks that file's final length, and charges the physical read/hash passes. It does not read indefinitely to EOF. The capture owner and SQLite read transaction establish the cooperative-reader boundary; they are not an immutable filesystem snapshot or protection against arbitrary noncooperative in-place writes or pathname replacement. This review does not qualify those hostile concurrent-writer cases. The sealed reader acquires no owner, so its source must already be treated as immutable by its owner.

## Service cancellation and lifetime

Cancellation must be an out-of-band atomic request for the currently correlated inspection; enqueuing a Cancel command behind the scan cannot interrupt it. Match service generation and request ID. The implementation reuses one private token per service generation, resetting it only after the prior correlated completion has released the sole command credit, and before publishing the next request. This is equivalent to fresh request state under the enforced one-worker/one-credit boundary; it does not expose an old token to callers. Late cancellation of request A cannot cancel request B. Retain the single command credit until the actual terminal completion arrives, including after cancellation. Do not detach the old scan, create a second storage worker, or reuse its connection merely because the UI stopped waiting.

Cancellation means “stop at the next checked boundary,” not “the source is closed now.” A blocked OS read/open, SQLite operation outside the hook's coverage, or filesystem close may delay completion. UI must retain busy/retiring truth and preserve exact pending context. Service drop signals cancellation but must not join on the UI thread. Reliable native Stop remains independently available. This source-only packet need not modify the current UI or package to expose future cancellation controls.

## Independent acceptance plan

- Pre-open: missing path/owner, live owner, old/future version, too-large physical file, WAL header, WAL/SHM/hot-journal sidecars, symlink/hardlink and path replacement. Compare exact file and directory bytes/listings; no repair, migration, sidecar creation or takeover.
- Budget-before-work: physical oversize wins before corrupt SQLite parsing; oversized field length wins before materialization; excess rows in bad/unselected tails and derived tables still count; growth after preflight cannot exceed remaining-plus-one reads. Tiny-row and tiny-record fixtures exercise non-byte work counters.
- Error semantics: cancellation or limit after a valid earlier checkpoint returns typed outer Err with no selection. A completely authenticated genuine semantic bad tail may return a clearly incomplete review selecting the earlier checkpoint. A semantic prefix does not waive the full-original digest.
- Cumulative accounting: manifest/generic/physical/original/semantic passes share usage. Use a small deterministic limit that fits each phase separately but not their sum. Test cancellation before open, during original-field blocks, between generic chunks, inside record/group processing and immediately before completion.
- Compatibility: bounded and legacy readers agree byte-for-byte on descriptor, physical/original hashes, manifest, index and selected checkpoint for valid v1/v2 normal and recovered fixtures within limits. Existing golden codecs/hashes remain unchanged. Normal/recovered mode and unknown-tail distinctions remain explicit.
- Malformed originals: rehashed conflicting batches/GUI records/clocks/checkpoints/Finish and nonfinite PCM remain rejected by the same semantic validator. Preserve full originals, including later unselected or corrupt rows; no recovery mutation is tested as part of this packet.
- Service: exactly one worker/credit, correlated cancellation, stale result/request isolation, nonfatal stage errors with subsequent healthy catalog operation, bounded directory discovery and DTOs. No engine Start/Finish, native input, attachment, intent append or recovery side effect.

## Independent test source

`crates/omniusong-recording/tests/bounded_inspection_review.rs` contains twelve independent synthetic cases against the frozen public API. They compare exact legacy identities in Recording/normal/recovered modes and both reader forms; require physical-limit rejection before corrupt-header parsing; retain all three sidecars; check sticky and post-prefix cancellation; count damaged-tail physical rows; enforce original chunk/work allowances across rereads/reopens; require typed VM/scalar-row refusal; reject views/generated columns; preserve opaque ordinary columns and bind changed unselected tails; and verify owner contention/release without creating a missing guard. All fixtures live under the repository's dedicated `.runtime/bounded-inspection-review/` folder. The author has not run Cargo or formatting. Their execution is included in the coordinator gates recorded below.

## Source audit before the integrated gate

The consequential VM finding is resolved in source: `recording/src/budget.rs::install` uses a one-instruction hook. The pinned SQLite implementation carries progress remainders per VDBE; reserving one 1,000-step allowance per connection would not account for many newly prepared short statements. The new hook explicitly incurs off-realtime overhead and claims no wall-clock latency. Its typed sticky stop is checked after database failures, so a SQLite interrupt does not become an ordinary damaged-tail report.

`inspection.rs` and `snapshot.rs` retain the same governor on their bounded handles. Both install it before schema/payload queries and preflight every physical row before prefix interpretation. Original hashing covers ordinary opaque columns and unselected tails. Their public bounded read/snapshot accessors check the governor around the inner result. The bounded capture handle refuses recovery and the bounded sealed handle refuses export, preventing a read-only request from escaping into those legacy mutations.

The engine's bounded semantic adapters require a bounded handle, preserve typed cancellation/limit errors, and check the governor after inner validators before selecting an earlier checkpoint. Existing ordinary corruption may still select a completed earlier group; cancellation or exhausted work cannot return that partial result as success. The request's one governor also covers the semantic rereads and repeated record decoding.

`session/src/performance_sources.rs` derives paths from the canonical capture namespace, separates 32-hex performance identities from generic hyphenated capture UUIDs, and counts invalid entries against discovery's 128-entry bound. Its review has no PCM/raw vectors and is serialized to a counting sink before allocating the hash input or publishing it. Busy, invalid, cancelled and over-limit sources map to nonfatal source errors; they do not trigger main-catalog retirement. `SessionService` retains pending credit during cancellation, only resets the token at a fresh admission, and signals cancellation without joining on drop. No additional source blocker was identified in these paths; the coordinator execution evidence is recorded below.

Separately, the coordinator assigned this reviewer the narrow `worker.rs` correction for the actual pinned-package error `Ended live scope summary disappeared` after recorded-performance playback. The worker now retains only already-announced same-worker performance progress until a new authenticated Ready replaces it. The parent's disappearance check is unchanged. Three device-free worker cases and two active private-peer cases (plus their ignored helper) cover retention, unannounced/foreign isolation, new-Ready replacement, and exact terminal rejection when a peer still omits the summary. This correction is authored here and requires the coordinator's independent review; it is not independent approval of this reviewer's own production change. None of these software fixtures establishes corrected native output acceptance.

## Integrated evidence and native boundaries

The reviewer read both complete gate receipts, verified every referenced command-log byte length and SHA-256, and independently counted their test-result lines:

| Receipt | Bound source snapshots | Results | Receipt SHA-256 |
| --- | ---: | --- | --- |
| `.runtime/rust-bounded-inspection/gate1-run.json` | 144 | 792 passed / 0 failed / 4 ignored; 60 groups | `57bd9148494c15ea99b8cb8ebf2e1e03436a0fffd073d7bcd1148c0c0c658d47` |
| `.runtime/rust-bounded-inspection/gate2-run.json` | 146 | 801 passed / 0 failed / 4 ignored; 61 groups | `7bf024ef8603ba11ace842229e96123543408ef73bbede78a1ae26ec06f063e4` |

These are immutable executed-source boundaries, not a claim that later diagnostic changes were already executed. The gate2 increase includes the Panic progress correction's six owner cases and three independent public-API cases. The latter also measure zero callback allocation/deallocation; source approval is recorded separately in `RUST-PANIC-PROGRESS-REVIEW.md`.

The coordinator's actual read-only inspection receipts report:

| Fixture | File bytes | Elapsed seconds | Charged work bytes / VM steps | Receipt SHA-256 |
| --- | ---: | ---: | --- | --- |
| `actual-capture.json` | 7,495,680 | 2.693927542 | 102,033,666 / 674,880 | `ddeb298c0362c375f7db0e6ce2a0d103a3f5b3ed3a370a621bda209454733975` |
| `actual-owned-sealed.json` | 7,495,680 | 6.181999167 | 214,843,800 / 1,448,351 | `a68b3f84f4bb1abd60e2aa023ededae503706acd62644fb9f9c47729b182e7a4` |
| `failed-retained-capture.json` | 9,703,424 | 2.613372958 | 88,895,917 / 601,291 | `d94f6db31b5a701507293b5f30d34fc74c376d4a473a9c6a41eaedb4fa6dbd34` |

All three receipts report exact source-file bytes and directory names unchanged. The owned sealed qualifier deliberately performs two snapshots; its cost is not one equivalent unsealed inspection. The failed retained source's review selects 1,104 callbacks / 565,248 frames and reports unknown tail. These receipts exercise bounded inspection, not normal sealing speed or real-time performance.

The coordinator reports actual `4981` Record/Finish/normal seal/explicit attachment/Undo/Redo/archive/restore/restart and fresh-generation playback success: 432,128 frames, 844 callbacks, 9.002666 seconds and one held voice explicitly cut. Its same-generation post-record Play failed the parent's missing-ended-scope check. The worker correction preserves that guard and retains only announced progress; subsequent native runs have not yet completed that exact corrected same-generation playback path.

The replacement `a948` recording failed `Recorded-performance live progress regressed`. Its retained checkpoint is 1,104 callbacks / 565,248 frames, while the last observed UI was 1,111 / 568,832: seven observed callbacks / 3,584 frames beyond the checkpoint. The retained source does not prove the specific race that occurred or any further unobserved tail. `native-regression.json` SHA-256 is `b16b68008612fa842e3b8e090b384d887ba818c21208bcf09d4f381f1f7b3a79`.

After the independently reviewed Panic correction, `ffc90e34130702ba9bf5615ec9a9a8cc834266d2653d1334ad0dcc4301ac7188` recorded 856,576 frames / 1,673 callbacks with actual keys/pads and reached Sealing, then retired with only the generic rejection visible. `finish-failure-source.json` (3,665 bytes, SHA-256 `aac5496134d2775d03714aece35cbfb7a8c4ecc498cb2080bc969a767e196784`) records the 14,716,928-byte retained journal, state Recording, no manifest and no attachment. It distinguishes original intent revision 1 from revision 2 containing the exact Finish identity. The source hash is `d94a8986ebfa149546d0b5c244117fe969d6a31fae1e5f33913684452729bcaf`. This does not establish which sealing phase triggered retirement. The approved next correction preserves an immutable bounded retirement cause before publishing retirement; it changes no deadline or seal authority and is not included in gate2.

Bounded source discovery/inspection is accepted at the two source gates above. The unresolved native Finish outcome, corrected same-generation playback, recovery admission/UI, retained-card workflows and long-session qualification remain separate requirements. No input device was opened. M2 and H09 remain open.
