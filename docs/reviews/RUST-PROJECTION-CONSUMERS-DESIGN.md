# Source-aware projection consumers

This is the settled design for the continuation from `5f849e5`. It opens the specific Library, read-only reader and take-attachment consumers described below. It is not an implementation or execution acceptance report. Musical schema 3, catalog schema 6, OSAR versions and immutable original ownership remain unchanged. The full native workstation goal remains open.

## Authority boundaries

There are three different results:

1. **Metadata admission:** a bounded stored document has valid schema, project identity/revision and registry shape. This can supply Library labels or an explicitly named metadata view. It does not authenticate original files or a claimed musical derivation.
2. **Current authenticated document:** the exact current document has passed original-source authentication, binding recomputation, geometry, assets and final registration/file checks under one held catalog snapshot and cumulative governor.
3. **Recorded attachment commitment:** an exact request, adjacent revisions and recorded result agree with the historical operation. This remains observable without original files. It does not make that historical document current or usable.

Neither a summary flag, metadata view, stored review, hash nor historical receipt can substitute for the second result. Generic Create still rejects nonempty projection ownership. Generic Save cannot insert, remove, reorder or rewrite the registry, downgrade it or promote an old opaque same-named field. The prior source-aware publication/edit/history and governed archive paths remain independent admission routes; no global guard is removed.

## Library and bounded metadata

Add `ProjectSummary.source_authentication_required: bool`. Set it to true only for a structurally valid current musical-schema-3 document with a nonempty owned registry. A false value is not a source-proof assertion. Existing malformed metadata remains a problem row with its diagnostic; a valid projection project is no longer labelled damaged merely because it requires source authentication.

`NativeStore::list_with_budget(&InspectionBudget)` owns a deferred `CatalogTransaction` and calls a new private consumer metadata helper. `list()` supplies the existing default inspection ceiling. Admit at most 512 summaries and explicitly refuse a 513th row rather than collecting an unbounded library or silently claiming completeness. Bound identity bytes before materialization, inspect stored payload lengths and prepay parsing/output allocation before fetching/decoding each document. All rows share the caller's governor. Preserve exact catalog/project identity and signed-revision checks. Genuine catalog SQL failures remain whole-operation failures, distinct from a document-level problem row.

`check_project_metadata_admission_with_budget(id, budget)` uses the same bounded decoder within a deferred scope and returns only `()`. Its default-budget wrapper remains available. It opens no take files and grants neither a revision reservation nor permission to publish arbitrary registry bytes. Attachment commit repeats its complete current-document and source checks.

Existing helpers to reuse are `read_document_metadata_with_budget`, `projection_metadata` prepayment and the strict core `ProjectDocument` parser. Do not materialize an unbounded generic JSON tree to decide which decoder to use. Do not infer source readiness from `problem.is_none()`.

## NativeReader

Catalog acquisition must preserve other SQLite connections' process-wide file locks. Open the eventual connection with `READ_ONLY | NO_MUTEX | NOFOLLOW`; do not open a separate raw catalog descriptor, even for a header probe. Before any schema SQL, borrow that connection's main `sqlite3_file` through `SQLITE_FCNTL_FILE_POINTER` and read the fixed 20-byte header with checked `xRead`. Keep the pointer local to the synchronous call, and let SQLite alone close its handle on success and every refusal path. This avoids the documented POSIX raw-close hazard without introducing a second probe connection or falsely declaring the live catalog immutable.

Require available version-1 I/O methods, the expected held-file length from `xFileSize`, and a successful unchanged `SQLITE_FCNTL_HAS_MOVED` result. Preserve returned SQLite error codes; explicitly refuse unsupported identity control rather than falling back to raw file access. Check ordinary nonsymlink metadata before acquisition and after validation, repeat held header/length/identity checks after the schema query, and reject a changed header, pathname or length. These are acquisition checks, not an immutable snapshot or source-authentication proof. A WAL header observed by the initial probe is refused before schema SQL and must retain the no-new-sidecar regression. This does not universally exclude an external journal-mode change between checks. The pinned default Windows VFS does not implement `HAS_MOVED`, so this Reader admission is unavailable there until a separate native identity implementation and platform gate qualify it.

Retain the old guarded `NativeReader::load_document` and short `read_snapshot` contract. Add two explicitly different methods:

```rust
pub fn load_document_metadata_with_budget(
    &self,
    id: &str,
    budget: &InspectionBudget,
) -> Result<ProjectDocument>;

pub fn load_document_with_budget(
    &mut self,
    id: &str,
    budget: &InspectionBudget,
) -> Result<ProjectDocument>;
```

The first is metadata only and must execute within the existing caller-held short snapshot; reject an idle connection if required to enforce that API contract. The second owns its entire deferred transaction and refuses nesting. It does not call the first through a nested transaction. It authenticates the current document only, not every retained history/operation/copy-proof snapshot.

The standalone authenticated path holds retained-file coordination and uses one budget for metadata, asset verification, registered-source opening, semantic derivation, geometry and final fences. Build a `RegisteredSourceSet`, open it with `RegisteredProjectionSources::open_collected_from`, then authenticate once per distinct performance original. Keep the source owner and proofs alive through `verify_registrations_from` and `verify_unchanged`. Verify every referenced legacy asset with `projection_publication::check_assets`. Finish the read transaction explicitly with detached hooks and rollback; preserve operation errors and cleanup usability information.

This reader is another handle to the same studio catalog, so its actual SQL errors are deliberately catalog-origin. The existing waveform worker converts them to its local result; it must not retire an unrelated reusable writer merely because a different reader connection failed cleanup. `archive_boundary::input` is not an isolation mechanism for nested `CatalogFailure`: it intentionally preserves live cleanup information. Source-journal failures retain their recording/source domain.

Waveform preparation currently reads a short document/asset/registration snapshot and then authenticates its selected raw source. It may use the explicitly named metadata method within that snapshot to find a clip in a projection project. It must retain exact project revision, clip address, take/manifest/stream/rate and current/original-window comparisons, plus existing lower waveform byte/chunk/channel/sample limits. A waveform is evidence about that selected original, not validation of every projection binding. Do not add full project authentication merely to repaint an unrelated audio waveform, and do not present its metadata view as a playable authenticated project. Existing waveform cancellation and raw-take verification are not automatically a new cumulative-governor qualification.

## Shared fresh attachment transaction

Root owns a separate attachment implementation because `active_projection_edits::save` commits internally and cannot atomically append a recovered source-action revision. The new normal `publish_take_with_budget(intent, budget)` and recovered source attachment share the following fresh-publication sequence for all supported musical schemas; the existing convenience methods dispatch to it:

1. Validate the exact publication request and reserved operation namespace. Perform bounded exact historical receipt lookup before source acquisition. Preserve the distinction between an absent receipt, damaged receipt and same-ID/different-request collision. Recovered attachment additionally compares the complete retained source-action history in the same snapshot.
2. For a fresh attempt, hold the immediate catalog scope, retained-file coordination and the same caller budget. Read strict bounded current metadata, check project revision, and for recovered attachment recheck the exact expected history/current action revision and unpublished receipt.
3. Construct the complete candidate by the existing `append_take_clip` operation. The registry must remain exactly equal, with `reject_opaque_promotion` still applied. Registry IDs remain reserved even after current notes or lanes were deleted. The incoming clip cannot smuggle a retained edit snapshot or alter derived-note ownership.
4. Collect the union of current and candidate take references. This includes retained projection originals whose current lanes/audio clips were removed and the incoming ordinary clip. Authenticate each distinct performance original once, validate current bindings, then validate candidate geometry after proving exact registry equality. A registered damaged original never falls back to a recovered working copy.
5. Capture the complete thirteen-table SQL baseline before mutation. Independently construct every expected project, history, operation and optional source-action/review change. Freeze the complete expected postimage before the first actual mutation.
6. Apply the project save, edit/cursor change, exact operation receipt and optional committed source-action append in the same live transaction. Preserve the existing complete source-history equality checks as a separate semantic fence.
7. Compare the full frozen SQL postimage, then check referenced assets, exact registered `TakeObject` values and held original identities/paths at the final boundary. Check the same governor immediately before commit. Construct the return value before commit; do no new source reads or cancellation checks after successful commit.

`RegisteredProjectionSources` and `ProjectionProofs` already supply source grouping, private held authority and final registration/file checks. Use explicit SQL-origin entry points where available so a private spool/source failure cannot be mistaken for a fatal main-catalog failure. Never reset the governor between current and candidate, between bindings or between SQL and source checks. Lower caller limits remain effective.

Normal `attach_capture` still has an ingestion boundary before project publication. Metadata preflight must occur before retaining a new object; final revision/source checks occur again during publication. A safely retained orphan after later conflict is not rollback of file retention and must not be deleted or replaced automatically. Any legacy ingestion step not migrated to the same budget must be described honestly as outside the new bounded publication scope. Recovered ingestion already uses its held `RecoveredCopyView` budget; preserve that view and action owner through the commit boundary.

## Independent SQL postimages

Extend the existing `PostimageLedger` with strict `expect_replace(before, after)` and `expect_remove(before)` operations. Each requires the exact table, typed primary key and full row digest already present in the captured baseline. Replacement must preserve the key unless explicitly modeled as remove plus insert. A mismatch, duplicate/conflicting plan or failed mutation poisons the builder. These operations never read the already-mutated destination to decide what was expected. Existing archive insert-only APIs keep their behavior.

The attachment overlay contains:

- replacement of the selected `projects` row with its exact planned payload, revision and cursor;
- removal of each exact future edit row pruned by branching;
- insertion of one adjacent before/after edit and the exact operation row;
- for recovered attachment, the independently prepared source-action revision and any legitimately absent immutable review row.

Read and prepay each removed row before adding its expected removal. Derive source-action expectations from `expected_history`, exact review/payload bytes and the same append validation rules; do not derive them from the row returned after a trigger ran. A small pure append planner or trigger-free private plan can share those rules. Keep the existing 32 action identities, 16 revisions, 65,536 review bytes and 16,384 action bytes. Prepay bounded history/review/payload processing before materialization; hard field limits alone are not cumulative accounting.

Freeze before applying SQL. Final verification includes unrelated projects, histories, operations, assets, take registrations, recording intents and source-action rows, as well as persistent schema. Valid same-count substitutions and trigger-created rows must fail. Retain the existing 512 MiB spool main-file and row/key/layout ceilings and the same cumulative governor; there is no allowance increase to accommodate a large catalog. Private staging cleanup cannot erase a committed fact. Failed staging is not replay authority.

The logical row scan first collects and bounds type/length headers without fetching numeric or variable-length values. Prepay each subsequent value read with twice the bounded complete row bytes plus 8,192, including a scalar after a large earlier field. TEXT/BLOB values use a SQLite-owned read-only incremental handle, exact header/handle length agreement, a buffer of at most 64 KiB, and sequential `read_at_exact` calls with separate buffer and per-chunk credits. Repeating a full-cell `substr(CAST(... AS BLOB))` for every chunk imposes avoidable quadratic work and is not the selected algorithm. Preserve storage-class tags, raw database-encoding bytes, numeric bits and typed key framing. The pinned rusqlite `blob` feature supplies this API without changing its package version.

This is a conservative logical-work allowance, not a literal pager-memory, physical-I/O or latency bound. Incremental Btree reads do not execute the SQL progress hook; check the same governor between chunks and do not claim interruption of one blocked pager read. No raw descriptor, writable handle, expired-handle retry or budget reset is allowed. Consume every opened handle with explicit close before the enclosing rollback/commit. Retain the original operation and an independent same-origin close error; a close-only failure must also have an explicit cleanup marker. Both private own-interrupt finalizers must preserve cleanup failures. Only a live-catalog origin can retire the catalog connection, preferring fatal codes and conservatively retiring after failed live cleanup.

For fresh attachment, the independently frozen operation row already contains the exact request, hashes, project identity, adjacent revisions and complete result bytes. Once `saved.document` equals the independently planned candidate, construct its fresh `EditReceipt` before the final full-table comparison and commit. Do not decode the just-written complete result again merely to reconstruct the same receipt. Exact historical receipt lookup remains fully checked and separate. A coherent replacement result with a matching replacement hash, and a silently ignored operation insert, must each fail the complete postimage fence.

## Historical receipts and session integration

Refactor the existing `checked_publication_receipt` semantic check to use bounded `read_operation_metadata`, preceded by operation prepayment, instead of guarded `replay_operation`. Preserve exact request bytes/hash, project and adjacent revisions, result hash, target recording track and exact clip checks. The creation-default track name is not a forced rename of an existing track. Keep copy/projection/source-attachment namespace collision rules and exact historical replay before fresh-ID restrictions where already required.

This lookup opens no original files and never installs its result as current. A later current view must use source-aware load. For the dedicated recovered source workflow, keep `committed`, `replayed`, `current_document` and `current_document_error` separate, with exact request correlation and nested fatal retirement. Stale historical choice replay cannot undo atomic retirement or substitute a replacement choice.

Normal historical replay has the same fact/current-usability distinction. After a later human edit, damaging or removing an original still referenced by the current arrangement must make active load refuse while the exact older receipt remains readable. Replay must leave the later raw project payload, cursor, history and all catalog tables unchanged. Conversely, structurally admissible metadata can permit normal legacy ingestion before later source authentication refuses a forged performance claim; retain that original and its exact registration, with no project operation receipt, rather than describing the failure as having produced no ingestion effects.

The normal SessionService `AttachCapture` path currently turns an `EditReceipt` into a generic document/library reply and may fail a later current load or library refresh. Source-independent store replay does not by itself qualify an explicit committed/current-usability session outcome for that command. Either retain this limitation explicitly or add the smallest typed commitment-plus-current-result envelope before claiming that behavior. Never silently install the historical result to hide a current-source error. Existing musical drafts, sole service credit, exact cancellation and native retry policies remain unchanged.

## Ownership and acceptance

The consumer/reader implementer owns the new bounded metadata/authentication module and the Reader methods; root owns public DTO/call-site changes, normal/recovered attachment integration, ledger CAS changes and contracts. Tests should exercise public routes with genuine device-free sealed performance fixtures, not synthetic review hashes used as authority.

Required cases include metadata listing with an absent original; valid projection summary versus malformed registry; 512/513 rows and oversized identity/payload refusal; cancelled/lowered-budget metadata reads; standalone Reader success, forged rehashed pitch refusal, absent/replaced original, one authentication per shared original and nested-scope refusal; waveform selection with lower existing limits; normal and recovered attachment success into an edited projection project; registry/note/original preservation; exact replay with absent originals; same-ID different request; stale project revision; source-action retirement/history mismatch before ingestion and at commit; future-history pruning; coherent trigger substitutions in selected and unrelated rows; `INSERT OR REPLACE` take registration changes; asset corruption; final held-path replacement; ledger CAS poisoning; actual VM cancellation/rollback and same-connection retry; no late cancellation replacing successful commit.

Reader acquisition also requires a real external-process writer-lock probe before and after successful open/drop and schema refusal, using no DML while the lock is held; deterministic path replacement after SQLite acquires its handle; malformed, short and invalid-journal headers with exact file preservation; and the existing WAL refusal without a new shared-memory sidecar. Same-process connection contention alone cannot establish preservation of the operating system's external lock.

Preserve the existing 2,000,000-byte opaque native-metadata fixture and the 1 GiB cumulative-work ceiling. Compare incremental bytes with SQLite's previous CAST-to-BLOB semantics for UTF-8 and both UTF-16 database encodings, including empty, NUL-containing and multi-chunk values. Exercise a tiny trailing scalar/text field after a large field, lower-work refusal before its value fetch, cancellation after an observed chunk, handle closure and fresh-budget reuse. Constructed close-only/dual-error controls establish origin, fatal priority and cleanup retention, not physical close-failure injection.

No test or native acceptance is asserted by this design. Native projection admission/confirmation, restart protocol, exact-binary consumer walkthrough, playback/listening, hardware input, MIDI 2/MPE, AI roles, plugins and Windows remain separate required work. This slice should remove these specific consumer obstacles through integrated authentication and atomic publication, not rename a guard removal as completion.
