# Bounded retained-performance inspection

This packet adds read-only inspection for retained performance sources. It does not authorize recovery, publication, capture resumption, or a normal Finish seal. Journal/profile versions and schema-5 catalog storage are unchanged. The pinned `4981d54a…` GUI bundle remains a separate candidate awaiting its actual walkthrough.

## One governor across every pass

`omniusong_recording::InspectionBudget` is public but not `Clone`. Its private shared state and cancellation token may be shared internally with a reader and its SQLite progress callback. A reader opened with a governor retains that exact governor for its lifetime; opening, hashing, generic scanning, manifest verification, semantic rereading, and record decoding cannot obtain fresh budgets between passes.

`InspectionBudget::new(limits: InspectionLimits, cancelled: Arc<AtomicBool>) -> Result<Self>` rejects limits above the compiled ceilings. Zero limits are permitted for refusal tests. `InspectionLimits` and `InspectionUsage` are strict serde structs with the following `u64` fields; usage fields omit the `max_` prefix. Defaults are these ceilings:

| Limit field | Ceiling | Accounting |
| --- | ---: | --- |
| `max_file_bytes` | 536870912 | Entire physical SQLite file, including free and unselected pages |
| `max_physical_rows` | 262144 | All rows in every application table, plus SQLite schema/sequence metadata, including damaged and unselected tails |
| `max_work_bytes` | 1073741824 | Conservative cumulative charges for bytes read, hashed, or decoded in every pass, including fixed metadata allowances; never unique-byte accounting |
| `max_row_visits` | 4194304 | Cumulative row and field visits, including metadata, NULL/scalar fields, and preflight |
| `max_chunk_visits` | 524288 | Every loaded/verified chunk, including repeated passes |
| `max_record_visits` | 2097152 | Every decoded raw event record, including repeated semantic decodes |
| `max_groups` | 65536 | Every semantic checkpoint group verified |
| `max_vm_steps` | 32000000 | SQLite progress-handler instruction budget, charged in fixed quanta before allowing further work |

Public governor methods are `checkpoint() -> Result<()>`, `usage() -> InspectionUsage`, `charge_work(bytes: u64) -> Result<()>`, `visit_rows(count: u64) -> Result<()>`, `visit_chunks(count: u64) -> Result<()>`, `visit_records(count: u64) -> Result<()>`, and `visit_groups(count: u64) -> Result<()>`. File/physical-row preflight and SQLite-hook installation remain recording-library internals. Counters use checked arithmetic. A first cancellation/limit failure is sticky; a later invalid row or SQLite error cannot replace it.

Typed outer errors are `RecordingError::InspectionCancelled` and `RecordingError::InspectionLimit { resource: InspectionResource, limit: u64, attempted: u64 }`. `InspectionResource` has `FileBytes`, `PhysicalRows`, `WorkBytes`, `RowVisits`, `ChunkVisits`, `RecordVisits`, `Groups`, and `VmSteps` variants. Cancellation and limits must escape every damaged-tail catcher as outer errors. A partial valid checkpoint is never returned as successful inspection when the governor refused the remaining work.

The 1 GiB cumulative-work ceiling does not promise that every 512 MiB file can be inspected: multiple authentication passes may exhaust it. Refusal retains the source intact. SQLite work is interruptible through the pinned rusqlite progress hook; filesystem open/read and other operating-system calls cannot be forcibly interrupted. The implemented hook charges one instruction at a time, with one initial instruction reservation per opened connection. This avoids replenishing a larger progress quantum for every short prepared statement and incurs off-realtime per-instruction overhead. No wall-clock deadline is claimed. Small review DTO serialization has its separate 64 KiB ceiling; usage reports the recording and semantic source passes.

## Reader entry points and schema guards

Add `CaptureReader::open_bounded(path, &InspectionBudget)` and `SealedJournal::open_bounded(path, &InspectionBudget)`. Both expose `inspection_budget() -> Option<&InspectionBudget>`; their existing chunk/ordinal/snapshot accessors automatically charge the retained governor. Calling a bounded semantic API with an unbounded reader is invalid. Legacy public APIs remain available for the existing trusted paths; these new guarantees apply only to bounded entry points.

Before SQLite sees a source, validate ordinary non-symlink, single-link files, rollback-mode SQLite header, physical size and absence of `-wal`, `-shm`, and `-journal` sidecars. A capture reader only locks an already existing ordinary owner file and never creates one. A sealed reader acquires no owner lock. After read-only open, install the governor before schema or payload queries, disable writes/mmap, and hold one read transaction. Reject unexpected tables, expected tables replaced by views or virtual tables, and generated/hidden columns before payload interpretation. Ordinary opaque extension columns remain included in the original-row hash; incompatible schema is rejected visibly rather than silently omitted. Preflight every physical row before generic scans, with bounded row iteration or cap+1 subqueries, never an unrestricted aggregate first.

## Semantic entry points

Add engine functions `inspect_performance_bounded(&CaptureReader)` and `inspect_sealed_performance_bounded(&SealedJournal)`, returning `omniusong_recording::Result<PerformanceInspection>`. They use the exact governor held by the reader. Charge record payload bytes and record counts before each contextual decode/allocation, including control members decoded again by group verification. Check cancellation between bounded units and after inner validators return errors, before converting ordinary corrupt tails to the existing diagnostic `problem`. Typed cancellation/limit errors remain outer errors. No semantic output grants native capture lifetime authority.

## Session service boundary

The first service packet is discovery and inspection only. Performance directory identities are exactly 32 lowercase hexadecimal digits, excluding all-zero. They remain separate from generic UUID capture stages. Discovery examines at most 128 total directory entries, counts skipped generic/unknown entries and reports truncation. A known exact stage may be inspected directly despite a truncated discovery list. The source path is derived under the canonical owned store's `captures` directory; no arbitrary path from the UI.

`ScanPerformanceSources` and `InspectPerformanceSource { stage_id }` are read-only. The inspection reply retains descriptor/full-original/physical/checkpoint identity and bounded usage, with a canonical review hash and a serialized reply ceiling of 64 KiB. It contains no PCM or raw event vectors. Corrupt/busy/oversize sources and cancellation are local source failures, never fatal catalog failures. No project selection or original output-device choice is invented for an orphan.

`SessionService::cancel_source_inspection(RequestId) -> bool` only sets the token for the exact pending inspection. It does not remove the pending request or restore worker credit. A second request stays Busy until the correlated completion arrives; stale cancellation cannot affect a newer request or generation. Dropping the service requests cancellation without blocking/joining. Existing backends remain compatible through a default cancellation-aware trait method. Cancellation is never applied to catalog mutations.

## Acceptance

Required checks include unchanged legacy inspection identities, both reader forms, cumulative repeat accounting, complete invalid-tail hashing, every limit class, cancellation before open and after a valid prefix, SQL VM refusal, unexpected/generated schema, all three pre-SQL sidecars, owner contention, no new sidecars or changed source bytes, request identity/credit handling, and healthy catalog use after refused source inspection. Production recovery, reviewed-action persistence, UI controls and actual native recording acceptance remain subsequent work.
