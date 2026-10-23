# Ordinary project archive outcomes

This contract covers the existing ordinary OSAR export, identity restore, restore-as-copy and header inspection routes. It adds completion/error semantics to those routes without admitting nonempty performance projections or changing archive, catalog or musical schemas. The native workstation goal remains open.

## Completion and cleanup

`export_archive_outcome`, `restore_archive_outcome`, `restore_archive_as_copy_outcome` and `inspect_archive_outcome` return `ArchiveOutcome<T>` or `ArchiveFailure`. Success carries the exact existing receipt/document, an explicit completion and a list of private cleanup issues, each with its concrete path and original I/O error.

Export prepares its complete receipt before the no-overwrite hard link. It records `OutputLinkAttempted` before that call, `OutputLinked` after success, and `ExportDurable` only after destination-parent synchronization. A link or parent-sync failure is an error with its observed phase, destination and retained staging path; it never claims durable completion. Once durable, private file/directory removal failures become warnings alongside the completed receipt. Export removes only its known private archive file and then its empty staging directory; unexpected private entries are retained.

Both restores prepare their complete return value before committing. `CatalogCommitAttempted` precedes the commit call, and `CatalogCommitted` follows successful commit immediately. No live-catalog SQL, source validation or fallible receipt construction follows a successful commit. Only private-input release and staging cleanup remain. A cleanup failure therefore cannot turn a committed project into an ordinary failure or suggest a fresh restore retry.

Exact restore-copy replay is `RecordedReplay`, not a new commit. The recorded receipt is constructed and authenticated inside a read transaction that explicitly rolls back before returning. Early replay needs neither the original input file nor a new project/operation identity. The second replay fence under the writer transaction retains the same semantics. Header inspection completes as `ReadOnlyCompleted`; it remains a bounded summary rather than complete restore validation.

The legacy four APIs delegate to these implementations. Their successful return values intentionally discard cleanup warnings because their existing return types have no warning field. Their errors preserve `StoreError::Archive(ArchiveFailure)`, and error text retains destination, source, staging, operation and confirmed-placement context for existing session/native consumers. Surfacing successful cleanup warnings in the native UI remains separate work.

## Failure facts and uncertainty

`ArchiveContext` retains action, destination, optional source/staging paths, optional copy operation identity, separately requested and observed archive hashes, confirmed filesystem placements and the latest phase. A hash is present only after the corresponding request validation or actual file acquisition/hash; identity restore adds no extra whole-file hash claim. Object placement is marked attempted before installation begins. A zero confirmed count does not prove an absence of filesystem side effects; SQL rollback does not remove retained content-addressed orphans. Failed attempts keep diagnostics and staging instead of guessing at repair.

Attempted output publication and attempted catalog commit are conservatively uncertain on error. A failed commit is not described as a proven non-commit. The session mapper preserves this uncertainty in addition to its existing conservative mutation policy. Typed phases are observed boundaries, not durable crash-reconciliation receipts or instructions to retry automatically. No new archive-job journal, adoption authority or crash-recovery guarantee is introduced here.

## SQLite provenance and rollback

`ArchiveSqlOrigin` distinguishes input SQLite, private export-output SQLite and the actual live catalog. Source/destination helpers tag errors at the specific handle operation. An outer input/output wrapper tags only a still-bare database error: it cannot relabel an already-tagged live error, a catalog rollback failure or an archive failure. The separately opened read-only `studio.sqlite3` used by export is actual live-catalog I/O.

Private input/output IOERR, CORRUPT or NOTADB codes remain available as diagnostics without claiming that the live catalog is damaged. Actual live-catalog codes and connection-unusable state survive nested archive failures. Existing fatal primary codes 10, 11 and 26 still retire the session worker. A failed live rollback independently makes the connection unusable, even when its primary operation failed in the private input domain and the rollback error itself has a nonfatal primary code.

Every actual archive live-catalog transaction uses an explicit scoped commit/rollback/error path. Its original failure, cleanup errors and active-transaction state remain separate. The legacy archive constructors install no SQL progress hooks and accept no inspection governor. Existing payload/row/lineage ceilings remain, but this work does not qualify cumulative archive cancellation, SQL-VM quotas or a hard time bound on filesystem/transaction-control calls. Private archive SQLite is never governed as a live-catalog connection.

## Verification and remaining work

Qualification uses isolated synthetic stores and actual ordinary export/restore/copy/replay paths, malformed private SQLite, real destination statement/trigger errors, real live-transaction rollback and denied transaction-control operations. Test-only thread-local hooks inject boundary/cleanup failures; those injected failures do not establish physical disk failure, crash durability or Windows behavior. Release/default builds contain no hook interface. Original live catalog/files and the installed app must be fingerprinted before and after execution.

Nonempty projection OSAR admission stays closed. Integrating the retained-source validator under one governed held context, final installed-path source authentication, exact expected destination postimages and a coherent three-route qualification still precedes that activation. Guarded reader/list/attachment integration, schema-3 inert evidence roundtrip, durable native admission and a named-binary native walkthrough remain distinct stages. This source change adds no package or native workflow acceptance.
