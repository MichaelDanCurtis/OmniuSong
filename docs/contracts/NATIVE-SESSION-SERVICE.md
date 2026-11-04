# Background native session service, revision 1

October 20, 2026. `omniusong-session` moves project storage, legacy scanning and
native backup work off the application's paint/input thread. It uses one
persistent native worker owning one `NativeStore`, plus bounded standard-library
channels. There is no web server, browser, JavaScript runtime or production
Python dependency. Dependencies are the existing native core/store, `serde_json`
and `uuid`. Features default to none; `test-hooks` enables explicit backend and
channel-peer injection for tests.

The service builds on [native edit receipts](NATIVE-EDITS.md),
[owned take storage](NATIVE-TAKE-STORE.md), [musical take references](NATIVE-TAKES.md)
and [complete native archives](NATIVE-ARCHIVE.md). Archive fingerprint inspection
is preliminary; full semantic/object/lineage validation still happens in restore.

## Ownership and startup

`Paths { native, legacy }` is a portable configuration DTO with no egui dependency.
Application construction only retains these paths. After the first loading UI
has been built, `SessionService::spawn(paths)` creates the worker and the UI submits
`Initialize`. `NativeStore::open`, schema migration, session listing and legacy
file reads occur inside that worker. The native event loop/window therefore
exists before a filesystem or external-drive permission decision can block.

The UI shows opening, busy, ready and failed/disconnected states. It polls while
work is active and requests repaint after 100 ms; it reports no invented
percentage or estimated completion. It explains that macOS may request drive
access through its own prompt. A failed initialization does not retry silently.
The explicit retry-opening or reconnect control starts the next attempt.

The service does not hold a `NativeStore` on the UI thread. All open/create/import,
save/history, backup export, inspection and copy restore operations run on the
same owning worker. Backend and connection destruction also run there. Public
`DocumentView::prepare` is a storage-thread preparation function: it moves the
document and prepares typed recording details and bounded provenance previews.
The UI does not parse/copy an 8 MiB project to save a name/tempo pair or deserialize
the recording payload in every paint call. No complete project document is part
of a command.

## Nonblocking transport and bounds

```rust,ignore
SessionService::spawn(Paths) -> Result<SessionService, ServiceError>
SessionService::reconnect(Paths) -> Result<SessionService, ServiceError>
service.try_submit(Command) -> Result<RequestId, Box<SubmitError>>
service.try_poll() -> Result<Option<Completion>, ServiceError>
service.is_busy() -> bool
service.is_disconnected() -> bool
service.pending() -> Option<&Request>
service.retry_command() -> Option<&Command>
```

Both request and response channels have capacity **one**. The client retains one
admission credit until it consumes that request's valid completion. The worker
removing a queued request does not release the credit: a caller that stops
draining replies cannot keep admitting work or accumulate response documents.
The UI uses only `try_send`/`try_recv`; it does not wait, receive blocking, sleep,
join a worker or perform filesystem I/O in paint, input or drop. The worker may
block receiving a request or inside a storage operation. Its response send is
nonblocking; an unexpected full/disconnected response channel ends that worker
rather than growing a queue or retrying indefinitely.

The boxed `SubmitError { kind, command, message }` returns the original rejected command on
`Busy`, `Full`, `Disconnected` or `Invalid`. Busy/full means that submission was
not admitted. Boxing keeps the normal result small and allocates only on a
rejected submission; the caller still owns the complete returned command.
UI controls prevent duplicate/navigation intents while a request
is active. Edit requests validate their core contract and a 64 KiB serialized
budget using a bounded counting writer before cloning/enqueueing. Paths are at
most 4,096 bytes and names at most 160 characters. Reply documents retain the
core's 8 MiB metadata ceiling; one request, one response and one current UI view
bound the number of outstanding documents. Legacy scans inspect at most 512
directory entries and read each JSON file through a `MAX_PROJECT_BYTES + 1`
limit. Schema-2 documents are excluded from legacy import. Sources are never
rewritten by scanning. The displayed native list is limited to 512 entries;
the underlying store list implementation may still read more rows on its worker.

Transport identities are `RequestId { generation, sequence }`. A new service has
a new generation; accepted requests advance sequence. These IDs are distinct
from durable `OperationId`s. Completion validation requires exact generation,
sequence, command kind and reply kind. A stale, cross-request or wrong-kind
response is rejected without releasing the real pending request or installing
its document. This is internal correlation for the owning thread/channel, not
authentication or a security boundary for an untrusted worker. The UI also
records project identity/revision and a small draft
snapshot at submission. Even a matching transport response cannot replace a
newer local document/draft context. Draft edits are disabled while busy, while
painting, inspection of the existing view and close remain responsive.

## Commands and replies

Retained capture commands extend this internal API without changing the project/database formats: `ScanCaptures` returns bounded diagnostics/stages; `InspectCapture { stage_id }` returns a boxed unchanged-byte review; `RecoverCapture { inspection, operation_id }` returns a boxed checked recovery receipt. `AttachCapture` remains the separate project publication command. Review/recovery never opens an input device. Exact reviewed fields, all retained original rows, stable retry identities and explicit interface choices follow [NATIVE-CAPTURE-RECOVERY](NATIVE-CAPTURE-RECOVERY.md).

| Command | Worker action |
| --- | --- |
| `Initialize` | Open/migrate native storage and scan read-only schema-1 legacy sources. |
| `Refresh` | Refresh native summaries and legacy scan; retain the open UI draft. |
| `Create { name }` | Generate and create a new native project. |
| `Open { project_id }` | Authenticate and load the saved project. |
| `ImportLegacy { path }` | Import through the checked legacy bridge and its assets directory. |
| `Edit { intent }` | Admit one validated global-ledger edit. |
| `AttachCapture { journal_path, publication }` | Authenticate a sealed stage/owned original and publish the exact stable project intent. |
| `ScanCaptures` | List at most 128 owned stage entries without opening or changing journals. |
| `InspectCapture { stage_id }` | Acquire a read-only exclusive review of retained source bytes and prefix. |
| `RecoverCapture { inspection, operation_id }` | Seal the explicitly reviewed prefix with checked original retention and idempotent receipt. |
| `ExportArchive { project_id, destination }` | Create a complete backup at a new path without overwriting. |
| `InspectArchive { path }` | Read the archive fingerprint and preliminary bounded summary. |
| `RestoreCopy { path, intent }` | Fully validate and restore as a new session using a stable operation and expected fingerprint. |

Replies are `Library`, `Document { view, library, replayed }`, `Exported`,
`Inspected`, `CaptureStages`, `CaptureInspected` or `CaptureRecovered`.
`Completion { id, kind, result }` binds one result to one request.
Library/legacy DTOs and commands have no egui dependency. The UI receives the
existing schema-2 recording inspector data and does not reinterpret opaque
`notes`, `audio`, `instrument` or `provenance` on a recording track. Both schema 2
and recording kind are required before interpreting a recording payload.

## Fatal storage failures and generation quarantine

The native backend classifies a typed `StoreError::Database` only when
rusqlite's `sqlite_error()` exposes `Error::SqliteFailure` with primary SQLite
code `SQLITE_IOERR` (10), `SQLITE_CORRUPT` (11) or `SQLITE_NOTADB` (26). The primary
byte includes extended members of those families. These names and numeric values
were checked against pinned rusqlite 0.40.2/libsqlite3-sys 0.38.2 sources.
There is no string matching, every-I/O rule or automatic repair command.

Such an error becomes `FailureKind::StorageUnavailable`. Mutating actions retain
the conservative uncertain-commit flag. The owning worker queues the one bounded
known failure response, then exits its loop and drops the database/backend on
that worker. It never dequeues another request using that connection. The
capacity-one admission credit prevents a new request being accepted while the
terminal response waits to be consumed.

The client first validates the terminal response's request generation, sequence
and kind. A stale fatal-looking response cannot poison an unrelated generation.
After consuming the matching failure, it marks the generation disconnected,
releases its pending slot and retains only a retry-safe command. It returns the
complete known completion before channel EOF. Later polls preserve that terminal
cause as an error rather than replacing it with generic disconnection. No
further submission is admitted to that generation. The UI keeps its document,
draft and known message, disables storage actions, and offers Close and explicit
reconnection. It does not clear the document or show an empty library as recovery.

Reconnection creates a new thread, generation and connection. After a successful
initialization, or a fatal failure opening an existing catalog, the UI uses
`SessionService::reconnect`. Its worker requires an ordinary nonsymlink
`studio.sqlite3` and calls `NativeStore::open_existing`: no directory creation,
SQLite `READ_WRITE | NO_MUTEX` without `CREATE`, and rejection of uninitialized
version-zero catalogs before migration. The no-create guarantee comes from
those SQLite flags, not a check-then-create filesystem assumption. Existing
supported catalog versions may still follow checked migration. All these checks
run on the worker. During reconnection, the loading/failure view is shown while
the cached document/draft remains retained in memory.

A deliberate first run that has never initialized and has not reported a fatal
catalog failure can explicitly retry normal `spawn` initialization and create
its selected new store. This does not solve broader bootstrap identity when a
first launch points at an absent/misidentified removable volume; volume identity
needs a separate design. Reconnection is not a forensic recovery API: it does
not execute `.recover`, rewrite originals or invent missing history/receipts.

Successful reconnection refreshes summaries while retaining the small draft and
safe durable intent. It does not automatically resubmit that intent. A second
explicit Retry action uses the same edit/copy operation ID with a fresh request
ID. Create, legacy import and archive export remain ineligible for blanket
mutation retry. Close/drop still performs no wait, join or storage access on UI.

Constraint, busy/locked, missing-project, semantic validation, collision, invalid
archive and missing external-input failures do not quarantine a healthy worker.
Existing archive wrappers convert some late errors to `StoreError::Invalid`
diagnostic text, losing typed origin. This version does not parse that text or
assume a malformed input archive damaged the main catalog. Typed main-connection
versus source-archive provenance must be preserved before broadening this rule.
Thus not all late wrapped storage failures are covered by this classification.

## Drafts, durable action IDs and retries

Saving name and tempo creates one `EditOperation::UpdateSession { name, tempo }`
inside a `ProjectEditIntent` with a validated unique operation ID. They share one
revision, history entry and Undo step. Undo and Redo also use `apply_edit` with
their own stable operation IDs. An explicit retry reuses the complete original
intent and ID but receives a fresh transport request ID. It never silently mints
a different save/undo/redo operation.

Backend storage errors conservatively report that a mutating action's outcome
may be unknown, including a failure delivering a post-commit acknowledgement.

The explicit exception is typed `StoreError::Conflict`: it becomes `FailureKind::RevisionConflict` with `outcome_unknown=false`, because the expected-revision project transaction did not publish. An original may already have been ingested before that separate transaction. Stage/journal SQLite faults remain nested recording errors and cannot retire the unrelated main catalog generation. Only a matched known conflict permits the interface's explicit rejected-choice release; uncertain failures retain their exact original request for suspended retry, without automatic rebase or replay.
Disconnection likewise does not prove rollback. `ServiceError` distinguishes
validation rejection, backend failure, disconnection and protocol mismatch, with
an `outcome_unknown` flag. The UI retains safe retry commands. A reconnect is
explicit and preserves the pending durable edit/copy intent through initialization.

Durable retries are supported for Edit, RestoreCopy and AttachCapture because
their stores retain receipts; RecoverCapture binds its exact reviewed request
to the recovered manifest operation. Read-only commands can be retried too. `Create`, legacy import
and backup export do **not** receive a blanket retry control: create/import can
generate a fresh project identity, and an export may already have published its
new filename. Their uncertain failures instead direct the user to refresh
sessions or inspect the output before choosing another action. No automatic
resubmission occurs.

An old successful ledger retry can refer to an older revision than the current
project. The native backend loads the actual latest saved project after a replay
before preparing the UI reply; it does not install the historical receipt over
newer music. The UI still guards its local context before installing that reply.

Invalid drafts remain editable and do not queue work. Unsaved navigation,
backup export/inspection or copy restore presents explicit Save, Discard and
Cancel choices. Save-and-continue queues the one atomic draft save, waits for its
acknowledgement, then submits the retained original continuation. It does not
immediately navigate or reset the draft. Discard is explicit; Cancel retains
the draft. A copy-restore intent is allocated before this dialog and survives
the continuation and any safe retry with the same operation ID and expected SHA.

## Backup flow and close behavior

The initial UI provides bounded path fields for creating a new backup, inspecting
an existing backup and restoring it as a separate session. A changed inspection
path invalidates its displayed fingerprint/summary. Restore uses exactly the
inspected path and full expected SHA. The summary is labelled preliminary; it
does not claim that all recording objects have passed validation. Full restore
performs those checks on the worker, preserves the input archive and existing
sessions, and returns the new copied project identity. The UI never exposes
identity-preserving overwrite as its restore flow. Underlying backup/database
implementation details are not presented as musician-facing success steps.

The manually recovered checkpoint has a case-specific disclosure only when its
root `omniusongOperatorRecovery` value declares format
`omniusong.operator-recovery-evidence`, version 1, and
`historicalLedgerComplete: false`. The UI says “Recovered snapshot · earlier edit
history is incomplete” and explains that this session starts new history while
the source checkpoint and recovery evidence remain in the project. Unknown or
nested lookalike metadata is preserved without displaying that notice or inferring
references. This is a disclosure of retained evidence, not an automatic recovery
capability. The `nativeImport` details use neutral source-identity/provenance
wording and do not assume that every native fork has a legacy import row.

Close is available during startup and every accepted action. It does not enqueue
a shutdown request behind a full channel, wait for storage, join the worker or
claim that accepted work was cancelled. Its message is: “Work is running in the
background. If the app closes, it may already have saved; reopening lets you
inspect the result.” Dropping the client closes its channel
ends only. An already running operation may finish or may be interrupted when
the process exits; the existing storage durability/receipt boundaries determine
its state. SQLite transactions, owned-object retention and explicit stable-ID
retry remain the recovery mechanisms. The nonjoining Drop test runs inside a
still-running process; it does not guarantee that application exit lets queued
or nondurable work finish. Closing is not a rollback acknowledgement or an
eventual-completion guarantee.

## Tests and remaining qualification

The opt-in `test-hooks` feature exposes `spawn_with_backend` and `test_pair`.
`Backend::handle(&Command)` runs on its worker; `TestPeer` exposes only the bounded
request receiver and response sender for identity/queue fault injection. No
custom backend injection is available through the production default constructor.
Test waits/gates belong to test threads and do not add production UI waits.

Implementation fixtures cover held startup with continued paint/accessible Close
and nonjoining Drop, native accessible create/save/Undo/Redo/reopen, invalid and
unsaved draft continuations, stable IDs after acknowledgement loss, newer local
draft rejection, real worker backup/inspect/copy, exact opaque recording metadata,
bounded schema-1-only scans, schema-owned recording preparation, and a committed
native edit retry after a newer edit. Independent review tests exercise channel
capacity/admission, stale generation/sequence/kind/reply rejection, disconnect,
held backend/drop and real worker behaviour. The coordinator owns final combined
test, formatting, Clippy and actual native-window evidence.

The storage-quarantine cases use an exclusively allocated synthetic store whose
catalog is deliberately truncated, checking that the worker drops on its own
thread, the known failure survives producer exit, further admission is rejected,
and a fresh existing-only reconnect sees the same unchanged damaged bytes.
Ordinary invalid archives, absent external input, missing projects and revision
conflicts remain usable in the same generation. UI regressions retain a dirty
draft and stable durable edit through fatal completion, explicit reconnect and a
separate retry; they also cover deliberate first-run retry and the exact manual
recovery disclosure without changing opaque project values.

This service removes filesystem blocking from UI callbacks; it does not establish
a storage deadline, audio realtime safety, power-loss survival beyond the storage
contract, or Windows permission/packaging acceptance. Native device capture,
plugins, playback, waveform decoding and model execution remain unavailable in
the application and are labelled accordingly. Worker cancellation, multi-window
session arbitration, verified object caches and very large-library pagination
need separate designs rather than unbounded queues or UI-thread fallbacks.
