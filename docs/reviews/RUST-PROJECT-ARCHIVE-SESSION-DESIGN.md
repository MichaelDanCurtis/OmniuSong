# Governed project archives through the session worker

This design adds the three existing governed OSAR operations to the sole `SessionService` without changing the ordinary native backup controls. The useful deliverable is an executable worker contract with compact, correlated completion facts, explicit cancellation and independent catalog retirement. It does not include durable native admission, projection publication controls, reader/list/attachment qualification or a new package. Those remain required work toward the full Mac-first Rust product.

This document is a source-derived implementation contract, not an execution report. Its author read repository Rust source and documentation; no Cargo, SQL, application, device, audio or protected-data operation was performed. The existing [governed archive source review](RUST-GOVERNED-PROJECTION-ARCHIVE-REVIEW.md) defines the underlying source/transaction limits.

## Existing behavior and the integration boundary

`omniusong-session/src/lib.rs` owns a capacity-one request channel, one pending request, generation/sequence identities, explicit cancellation tokens and the only `NativeBackend`. It uses Rust values over an in-process channel; the entire `Command`/`Reply` family is not a serialized process protocol. The new payloads nevertheless have strict serializable DTOs and measured byte limits so they can be retained, tested and transported without depending on Rust layout. This change does not silently expand the core worker-envelope limit.

The present `ExportArchive`, `InspectArchive` and `RestoreCopy` handlers use the legacy store family. RestoreCopy calls `load` on replay and then `document`, which calls `library`. Either later read can fail after the copy committed. They also expose only a flattened `ServiceError` or ordinary `Reply::Document`, losing the new outcome/cleanup distinction. The new handler must not copy that pattern.

The native `main.rs` backup controls currently create those three legacy commands and install a successful Document reply into the current editor, subject to draft checks. They remain unchanged. `SessionState::submit` explicitly rejects `RunProjectArchive` before taking credit, assigning a native action or altering drafts. An exhaustive new Reply arm must not install a document or start a continuation. This is an enforced delivery boundary, not a claim that the final user workflow needs no controls.

The current governed store API has three important limits that a session wrapper cannot invent away:

* Identity restore accepts an input path but no expected archive SHA. It authenticates its acquired bytes; it is not a reviewed-hash-pinned identity restore.
* Export selects the current saved project snapshot by ID. It has no expected project revision or expected document digest parameter.
* Existing `inspect_archive_outcome` remains a preliminary, legacy header/fingerprint route, without this shared governor or complete projection-source proof. There is no new bounded Inspect command in this slice.

## Public request and reply

Add `mod project_archives` and these two outer variants:

```rust
Command::RunProjectArchive { request: Box<ProjectArchiveRequest> }
CommandKind::RunProjectArchive
Reply::ProjectArchive(Box<ProjectArchiveReply>)
```

The request is version 1, with no generic JSON extension payload:

```rust
pub struct ProjectArchiveRequest {
    pub version: u32,
    pub origin_root: PathBuf,
    pub operation: ProjectArchiveOperation,
}
pub enum ProjectArchiveOperation {
    Export { project_id: ProjectId, destination: PathBuf },
    RestoreIdentity { archive_path: PathBuf },
    RestoreCopy { archive_path: PathBuf, intent: RestoreCopyIntent },
}
pub struct ProjectArchiveReply {
    pub version: u32,
    pub request_sha256: String,
    pub invoked: bool,
    pub outcome: ProjectArchiveOutcome,
    pub catalog_retirement: Option<ProjectArchiveCatalogRetirement>,
}
```

`invoked` means the actual store method was called. It does not mean a fresh copy was created: exact copy replay also has `invoked=true`. After response/correlation work has been reserved, a valid request rejected by missing store, origin, parent-path or budget preflight has a typed `Failed` outcome and `invoked=false`. A structurally malformed request is rejected before submission or, for a direct backend caller, before store access; it does not enter path conversion. There is one additional pre-effect outer-error boundary: failure to establish/pay the initial response reserve returns a typed Cancelled/Limit ServiceError with `outcome_unknown=false`, before request hashing or store access. It cannot be forced into a fingerprinted DTO by doing unpaid work after the stop.

Use four finite outcome variants:

```rust
pub enum ProjectArchiveOutcome {
    Exported {
        receipt: ProjectArchiveExportReceipt,
        cleanup: ProjectArchiveCleanup,
    },
    RestoredIdentity {
        project: ProjectArchiveProject,
        cleanup: ProjectArchiveCleanup,
    },
    RestoredCopy {
        operation_id: OperationId,
        source_project_id: ProjectId,
        source_archive_sha256: String,
        project: ProjectArchiveProject,
        completion: ProjectArchiveCopyCompletion,
        cleanup: ProjectArchiveCleanup,
    },
    Failed {
        context: ProjectArchiveContext,
        problem: ProjectArchiveProblem,
    },
}
pub struct ProjectArchiveProject { pub project_id: ProjectId, pub revision: u64 }
pub enum ProjectArchiveCopyCompletion { CatalogCommitted, RecordedReplay }
```

Exported intrinsically denotes `ExportDurable`; RestoredIdentity intrinsically denotes `CatalogCommitted`. Do not add an independently mutable completion field that can contradict those variants. ExportReceipt copies `ArchiveReceipt`'s exact path/project/SHA/byte length and history/operation/asset/take counts, using fixed `u64` counts. RestoredCopy preserves the exact requested operation ID and archive pin; source and destination project IDs remain separate. No ProjectDocument, Library, full proof JSON or current playback view is in this reply.

For identity success, the current store return does not contain the acquired archive digest. Do not reopen or hash the input after commitment to fill a new field. The exact request fingerprint binds its path; the reply makes no additional digest claim. Similarly, derive project identity/revision from the already returned document, without serializing the whole document after success.

Use strict serde structures (`deny_unknown_fields`) and explicitly tagged, snake_case enums. Store error types and `ServiceError` need not acquire Serialize implementations.

## Failure, cleanup and retirement facts

`ProjectArchiveContext` mirrors the store's action, phase, optional staging/source paths, destination, optional operation ID, requested archive SHA, observed archive SHA and confirmed object count. Use finite DTO action/phase enums, rather than string parsing. Rename the count `confirmed_objects: u64` to emphasize that zero is not proof of no filesystem effects. All observed phases survive serialization, including `RecordedReplay` and attempted phases.

```rust
pub struct ProjectArchiveProblem {
    pub kind: ProjectArchiveProblemKind,
    pub detail: ProjectArchiveMessage,
    pub sqlite: Option<ProjectArchiveSqlProblem>,
    pub governor_stop: Option<ProjectArchiveBudgetStop>,
}
pub enum ProjectArchiveProblemKind {
    Rejected, Conflict, Cancelled, Limit, Busy, Unavailable, Io, Sqlite, Other,
}
pub struct ProjectArchiveSqlProblem {
    pub origin: ProjectArchiveSqlOrigin,
    pub extended_code: i32,
}
pub enum ProjectArchiveSqlOrigin { Input, PrivateOutput, LiveCatalog, OwnedSource }
pub enum ProjectArchiveBudgetStop {
    Cancelled,
    Limit { resource: InspectionResource, limit: u64, attempted: u64 },
}
pub struct ProjectArchiveMessage { pub text: String, pub truncated: bool }
pub struct ProjectArchiveCatalogRetirement {
    pub sqlite_code: Option<i32>,
    pub connection_unusable: bool,
    pub detail: ProjectArchiveMessage,
}
```

Map the original operation recursively through Archive/Catalog envelopes. Explicit archive SQL tags override only their own SQL operation; recording database errors have OwnedSource origin. A nested Catalog cleanup error is a different fact from the original operation error. Before truncating or reducing the original failure, independently derive retirement from its recursive live catalog code/unusable accessors: primary IOERR, CORRUPT or NOTADB, or an unusable live connection. For example, an original source NOTADB plus a live rollback IOERR retains the source problem and independently requires retirement. A private archive NOTADB alone cannot retire the studio. No message, pathname or phase heuristic is allowed.

The original problem and a separately observed governor stop may coexist. Preserve both; do not promote a later cancellation into the operation cause. If the operation itself is a typed stop, retain that stop even when no later stop was attached. The lower recording finalizer limitation documented in the source review remains; this mapper cannot recover an error that never reached it.

Cleanup is only post-completion private cleanup:

```rust
pub struct ProjectArchiveCleanup {
    pub issues: Vec<ProjectArchiveCleanupIssue>,
    pub omitted: u64,
}
pub struct ProjectArchiveCleanupIssue {
    pub path: PathBuf,
    pub detail: ProjectArchiveMessage,
    pub raw_os_error: Option<i32>,
}
```

Current governed routes produce at most three issues. Cap the DTO at four and count any future omissions explicitly. A cleanup warning cannot become an operation failure or erase a commit. Live rollback/detachment failure is never placed in this cleanup array.

Interpret failure phases as follows:

| Observed phase | What the caller may conclude |
| --- | --- |
| NotStarted with `invoked=false` | The route was not called. The admitted request is still exact. |
| NotStarted with `invoked=true` | No publication boundary is recorded; private acquisition/staging may exist. |
| ObjectsPlacementAttempted / ObjectsPlaced | No catalog commit is recorded; unregistered objects or staging may remain. Zero confirmed objects does not prove none were placed. |
| OutputLinkAttempted | Output existence/durability is uncertain. Never invent a new destination as recovery. |
| OutputLinked | Link creation was observed; directory durability has not been confirmed. |
| CatalogCommitAttempted | Commitment is uncertain; no rollback claim follows from the error alone. |
| ExportDurable / CatalogCommitted | That completed fact remains authoritative even if a later diagnostic is present. |
| RecordedReplay | The exact recorded copy identity was found, but current owned closure may be unavailable. This is not evidence that the original copy failed or permission to make a replacement copy. |

Transport EOF/panic is a separate absence of a correlated terminal fact. Retain the exact request and report outcome unknown for this mutating command; do not synthesize a precommit phase.

## Bounds and exact correlation

Compile these limits in the DTO module:

| Item | Limit |
| --- | --- |
| Canonical encoded request | 65,536 bytes |
| Encoded reply | 262,144 bytes |
| Admitted path text | 4,096 UTF-8 bytes |
| Generated diagnostic/cleanup path | 8,192 UTF-8 bytes |
| Message text | 1,024 UTF-8 bytes, with explicit truncation |
| Cleanup entries | 4, plus omitted count |
| IDs and SHA | Existing <=96-byte ID validation; lowercase 64-byte SHA |

The reply ceiling is separate from the core 65,536-byte worker payload. It accounts for worst-case JSON escaping of four complete generated cleanup paths and bounded messages plus an export receipt, or all failure-context paths. Do not duplicate the whole request in the reply. Apply a bounded writer before allocating encoded output. Reply `from_json` checks raw length and strict syntax before returning a parsed DTO; it deliberately does not discard a matching-reference body's independently valid retirement merely because another field is semantically invalid. The caller then checks exact reference, extracts retirement and calls `validate_for` in that order. Unknown fields/versions and invalid cross-field combinations never become accepted results.

Admitted paths are absolute UTF-8 with exact lexical spelling: no ParentDir or normalization aliases. Compare OS text, not normalized Path equality. If control characters are refused, make that a request rule rather than silently sanitizing a path. Before export effects, require an ordinary canonical existing parent and exact canonical-parent-plus-filename equality to the requested destination. Require `request.origin_root` to exactly equal the actual store's canonical root. The runner does not create or lazily open a store to satisfy a request.

All real conversion paths originate from an admitted request and that matching root. Their generated paths are bounded root/input plus fixed ASCII suffixes. Conversion helpers are crate-private and preserve exact paths; they never manufacture a truncated or empty replacement path. A malformed request is rejected before these helpers. Unexpected malformed test/backend bodies are protocol failures, not usable paths.

`ProjectArchiveRequest::{validate, canonical_bytes, fingerprint}` produce deterministic field-ordered strict JSON and SHA-256 over the complete version/origin/operation/path/project/copy-ID/pin request. It is a request identity, not a source proof or durable job admission. The outer generation/sequence remains necessary.

Separate two reply predicates:

1. `matches_request_reference` checks reply version and exact request fingerprint. Together with outer RequestId and CommandKind, this runs before releasing pending credit. A mismatch leaves the original pending request/credit intact.
2. `validate_for` checks the finite outcome shape, exact export destination/project, exact copy operation/archive pin, legal completion/phase/context combination, limits and bounded retirement shape. A matching reference with an invalid body is consumed as Protocol/outcome-unknown without installing proof or document. Preserve any independently valid retirement requirement before replacing the body; malformed semantics cannot keep an unsafe generation alive.

The caller retains its original command, including the exact copy identity. Neither generic retry nor a new operation ID is a response to uncertainty. `retry_safe` is false for the new command, including copy. Explicit exact copy replay is possible through the backend API, but a future native continuation requires its own persisted admission and review state.

## Worker implementation order

The runner is `project_archives/runner.rs`, with `NativeBackend::run_project_archive(&ProjectArchiveRequest, Arc<AtomicBool>) -> Result<Reply, ServiceError>`.

1. Validate the request without opening storage. A healthy/project-usable summary is not an extra prerequisite: Initialize normally returns a Library whose projection-bearing entry carries `ProjectSummary.problem` from the intentionally closed list guard. The new runner uses the actual opened store without weakening that guard. It also does not introduce an additional Library read as an admission condition.
2. Construct one `InspectionBudget` using compiled limits and this command's token. Reserve response conversion work under that same governor before invoking a route: eight reply-sized passes, 2 MiB at the stated ceiling, plus separately bounded request/correlation credit. Then derive the exact fingerprint, require an already present actual NativeStore and its exact canonical origin, and perform the bounded filesystem preflight. An initial reserve stop uses the pre-effect outer exception above; subsequent preflight stops use the already-paid typed reply.
3. Invoke precisely one of the three existing bounded outcome methods with that same budget. No new default budget per source, phase or replay. The copy route retains its existing replay-first semantics inside the governed store method; session code must not acquire the input or allocate an alternative operation before that method.
4. Consume its result into a compact DTO. Successful conversion extracts existing receipt/id/revision fields only. It performs no load, library scan, input rehash, document serialization, new cancellation check or semantic source revalidation after the store returns success. The bounded conversion reserve is already paid.
5. For failure, map the original ArchiveFailure and independent stop/retirement as above. For preflight failure, use admitted request fields and a NotStarted context with `invoked=false`. The normal archive failure is an `Ok(Reply::ProjectArchive(...Failed...))`, so the generic mutating-error mapper cannot flatten its phase.
6. Queue the correlated reply, then stop the worker if its retirement DTO demands it. `terminal_storage_failure` adapts that DTO into a StorageUnavailable ServiceError without adding serde to ServiceError. The client records the same terminal generation state after exact correlation.

Build diagnostic text directly through a bounded `fmt::Write` sink. Calling `error.to_string()` and truncating afterward allocates the full diagnostic before its cap. Completed result conversion must be infallible for admitted typed fields; a reply serializer failure cannot be used to turn a known commit into a generic rejected operation.

Add a distinct `project_archive_cancelled: Arc<AtomicBool>` and a compatible Backend entrypoint that delegates older commands to the existing cancellation chain. NativeBackend intercepts only the new family. `cancel_project_archive(RequestId)` succeeds only for the exact pending identity/family, sets that token and retains credit. Reset tokens only while credit is free before publishing the next request. Drop does not implicitly cancel this accepted mutating family. Explicit cancellation requests cessation, never rollback; worker/disconnect behavior stays nonblocking.

## Native preservation and later admission

This slice's raw native rejection needs a real coordinator regression: submitting any new operation through an unrelated Action or continuation must leave service credit, active owner, current document, metadata/note/clip drafts, retained edit retry and archive review unchanged. No new button is added, and no existing button changes API family.

The later native route must durably retain the exact versioned request/fingerprint/origin and operation IDs before attempting effects; preserve unresolved requests across restart; offer exact reconciliation/replay rather than fresh identity allocation; and carry typed cleanup/phase/retirement warnings visibly. It must add a qualified source-aware inspection/review route and address identity hash/export revision confirmation explicitly. Draft/audio/source/evidence barriers and generation/context invalidation remain mandatory. Source-aware reader/list/attachment routes and installed-document rendering must be qualified before treating a restored projection-bearing project as an active editor view.

This is a finite follow-on requirement, not a reason to delay the backend contract or expand this slice into a generic job framework. It also does not qualify durable projection publication, native performance editing/audition, Windows, MIDI 2/MPE transport or the complete workstation milestone.

## Consequential acceptance cases and ownership

The following are implementation tests to execute, not results asserted by this design:

* DTO roundtrip and canonical fingerprint identity; strict versions/unknown fields; raw path aliases, UTF-8 and byte bounds; quote-heavy paths at the encoded cap; every phase and requested/observed hash distinction; cleanup omission accounting; bounded formatting without a full intermediate string.
* Actual NativeBackend export, identity restore and copy against closed synthetic stores; copied operation ID/archive pin survive exact replay; no library/load call occurs after commitment. A projection-bearing opened store with a guarded `ProjectSummary.problem` can still run the explicit bounded command; that summary does not grant source-aware list authority.
* Same-size forged replies with wrong root/path/project/copy ID/hash have exact reference or semantic outcomes as specified. Wrong outer generation/sequence/kind/fingerprint never releases credit. Matching malformed body consumes credit as uncertain Protocol, preserves exact caller request, and does not lose valid nested retirement.
* Exact cancel token and stale cancellation refusal; saturation/one credit; zero-budget preflight never invokes the store; VM/work stop and same-store fresh-governor reuse; no fresh governor inside source work; cancellation after actual publication does not erase completion.
* Actual private corruption remains local. Injected fatal-code matrices are labeled injection; actual SQL/trigger tests supplement them. Source error plus main cleanup fatal retains both original problem and retirement; the worker queues that one completion and accepts no further command.
* Known RecordedReplay plus missing owned source stays a recorded refusal without input acquisition, new project/operation IDs or automatic retry. EOF after accepted export/identity/copy stays unknown; there is no claimed durable native recovery yet.
* Cleanup warning retains success; result conversion does not touch disk after commitment. Legacy command behavior and all existing session/evidence/source action tests remain intact.
* Native raw dispatch refuses the new family before credit/action/draft changes, including an injected continuation and an already retained musical retry.

The DTO implementer owns only `omniusong-session/src/project_archives.rs` and its child DTO tests. Root owns `project_archives/runner.rs`, session `lib.rs` integration, backend/worker tests, app raw rejection/tests, contracts and verification. This architecture author owns only this design document and subsequently performs independent source review. No store migration, new source-authority token, global guard removal or extra service thread is required.
