# Native edit admission and durable receipts

October 20, 2026. `NativeStore::apply_edit(&ProjectEditIntent) -> EditReceipt` admits project rename, tempo, paired `UpdateSession { name, tempo }`, undo and redo. The pair commits one revision, one history step and one receipt. `publish_take(&TakePublication) -> EditReceipt` uses the same global operation ledger for explicit [sealed native take publication](NATIVE-TAKES.md). Both follow the operation-ledger boundary in [native protocols](NATIVE-PROTOCOLS.md). Prepared semantic edit objects and candidate acceptance return an explicit unavailable error; no inference, scope/lock or candidate gate is passed by these APIs.

## Transaction and retry behavior

The request is validated against the versioned core contract and its 64 KiB ceiling. Serialization uses deterministic struct order and sorted extension maps; unfamiliar numbers retain their exact JSON spelling. Numeric spellings such as `1` and `1.0` are different request content. The caller generates one operation ID per intended action and retains it when retrying that action.

One immediate SQLite transaction checks the global operation ledger. If the ID already has identical request content, it validates the retained request/result checksums and identity/revision relationship, then returns the original committed snapshot with `replayed = true`. This happens before the current project revision check. It does not reapply the edit or overwrite newer music. Callers displaying the current session must separately load the latest revision rather than install an old replay result over a newer local view.

An ID reused for different request content or a different project is rejected. A new ID must match the current project revision. The implementation uses the same save/history helpers as ordinary native edits, then inserts the operation request, result snapshot and checksums in that transaction. A receipt is returned only after commit. Failure at the final receipt insertion rolls back project changes, history/cursor changes and the receipt together.

Undo and redo produce new project revisions; their operation receipts remain retained even when a later edit truncates the redo branch. Retrying an old undo returns that undo's original result rather than undoing a second time. Receipt snapshots and their source assets remain retention roots. No automatic garbage collection or receipt deletion is implemented.

The ledger detects accidental corruption through checksums and consistency checks. It is not an authenticated database against an attacker who can rewrite both payload and checksum. Main-store transactions remain separate from engine activation, audible playback and recording journal commit receipts.

## Store migration

The native **SQLite schema version** advances from 1 to 2 by adding `operations`, from 2 to 3 by adding the immutable `native_takes` registry, and from 3 to 4 by adding [copy lineage/admission](NATIVE-ARCHIVE-COPY.md). Projects, assets, imports and history retain their existing tables and bytes. Immutable lineage triggers and cross-kind global ID guards protect copy admissions and ordinary operations. Migration rechecks the version under an immediate transaction, so simultaneous first-open/migration cannot independently publish competing schemas. Unknown newer stores are rejected.

This database version is distinct from the musical document's `schemaVersion`. Existing projects and imports remain schema 1. Publication explicitly upgrades a schema-1 document to musical schema 2, then appends a typed recording clip to an unlocked recording track or creates that track. The resolver checks the owned original, exact manifest/stream, selected source prefix, range units and audio sample rate. Unknown legacy fields remain opaque. The schema-2 metadata limit is 8 MiB; long originals remain external chunked take objects. Recording journal creation, sealing and ingestion alone do not attach a track to a project.

Ingestion completes a durable SQLite backup before registering an owned content-addressed take file. Publication's deterministic request is a version-1 `take_publication` wrapper containing `TakePublication`; there is no external source path in that request. A matching retry can return its original committed result after the external source is unavailable, because the store retains and verifies its owned original. Cross-kind or cross-project operation-ID reuse is rejected. Files committed before a later registry/publication failure are retained as recoverable orphans; no automatic deletion is implemented.

## Integration limits

The native draft editor now submits one `UpdateSession` intent through the [background session service](NATIVE-SESSION-SERVICE.md); save, Undo and Redo retain stable durable action IDs for explicit retries. The direct save/history API remains available for callers and migration fixtures. Prepared edits, candidate ledger and recording/capture integration remain pending. An IPC/session-service request ID is not a durable operation ID.

Complete native archives retain operation and copy-proof results even after redo history is truncated, because those snapshots can be the last reference to an original. Identity-preserving restore carries the same live receipts; copy restore remaps typed identities and retains all immutable ancestry. Both enforce the live operation namespace. See [native archives](NATIVE-ARCHIVE.md).

Meaningful tests must cover identical/conflicting retries, intervening edits, redo branching, concurrent connections, final-statement failure, receipt corruption and migration with retained data. Actual process/power failures and the complete recording/candidate workflow remain separate acceptance requirements.
