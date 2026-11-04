# Scoped live-catalog SQL governor

This is a Rust store prerequisite for performance-to-note jobs. It adds no native control, inference, musical schema change or installed package. Procedural projection remains distinct from AI inference. The full workstation release goal remains open.

## Qualified boundaries

The store owns its reusable main `Connection` privately. `CatalogTransaction` scopes one progress handler and one rollback observer to a single transaction. No raw transaction or mutable connection escapes the scope. A pending transaction is refused before replacing hooks. The owning session worker must discard a connection reported unusable rather than retry work on it.

The progress callback receives only an owned opaque callback sharing `InspectionBudget`'s private cumulative state. It charges one VM step per callback at SQLite's one-instruction interval, observes sticky cancellation and retains the first budget stop. It does not reenter or modify the connection. Sources continue to use their own held read handles with this same governor; catalog work does not reset source work or raise the compiled ceilings.

The scope covers metadata dispatch and active projection load, source-authenticated publication, unchanged-registry generic save/edit and Undo/Redo. SQL preparation, queries, DML and triggers inside these scopes receive the governor. Ordinary legacy branches explicitly detach before their established unbounded file/SQL paths. Other metadata-reader, archive, attachment and recovery consumers retain their existing guards and qualification; this contract does not silently qualify them.

Generic save/edit/history methods currently create their own bounded default budget. `load_with_budget` and typed publication accept the caller's governor. This is not yet a native job/cancellation API for every musical command.

## Receipt exception

An exact compact publication receipt is a recorded catalog fact, not current playable source authority. Its lookup uses a separate bounded metadata governor before caller cancellation, source access or retained-file acquisition. A matching recorded commit returns without reinstalling a snapshot, even if the caller is cancelled or originals are absent. On a miss, the same still-open transaction replaces that metadata governor with the caller's cumulative governor before source authentication or mutation. It never resets the caller's counters. Malformed or mismatched metadata still refuses.

## Transaction and cleanup fences

Fixed `BEGIN`, `COMMIT` and explicit `ROLLBACK` execute with the governor detached. `BEGIN` is preceded by a budget checkpoint. Installing a callback before it can leave SQLite inside a transaction even if a final callback makes the API report interruption before Rust receives a transaction owner.

Before `COMMIT`, the complete result and validation are prepared and the last budget checkpoint runs. Both scoped hooks are detached before the fixed commit statement. A successful commit returns that prepared result with no fallible validation or cancellation checkpoint afterward. Late cancellation cannot turn an actual committed save into a reported cancellation. A failed commit remains an error and receives cleanup.

Every normal error detaches both hooks before explicit rollback. Automatic rollback inside SQLite may already have ended the transaction; the scope checks `is_autocommit` before issuing another rollback. A rollback observer only disables the callback's atomic flag so progress catch-up during automatic cleanup cannot keep interrupting it. Scope `Drop` provides detach/rollback during unwinding; it is a panic backstop, not proof of recovery from failed I/O during unwinding.

Normal cleanup failure retains the original operation and all returned cleanup errors in `CatalogFailure`, including whether the transaction remains active. Its connection is unusable. Session storage classification retires such a worker even when a cleanup code is not one of the ordinary fatal catalog codes. The store does not pretend a denied rollback succeeded.

The scope's mapper converts only a returned `SQLITE_INTERRUPT` requested by its own callback to the typed recording governor stop. Existing source helpers retain their separate sticky-stop mapping for other nonfatal errors. Returned live-catalog I/O, corruption and not-a-database codes retain their origin and fatal priority. Errors from an owned original's separate SQLite handle stay source-local. Evidence failure classification preserves that distinction.

## Limits

This is a cumulative SQLite instruction and cooperative cancellation boundary. Busy waits, filesystem stalls, locking, fixed transaction-control fences and arbitrary native SQLite work do not have a hard elapsed-time limit. The one-instruction callback incurs off-realtime overhead. No realtime audio callback uses this scope.

Hooks assume the store's private connection has no unrelated progress or rollback observer. Nesting is refused; there is no public callback stacking or restoration contract. A normal cleanup error is returned for connection retirement; failed panic cleanup and physical disconnect/power-loss durability need separate qualification.

Real isolated SQLite tests cover query/trigger interruption, cancellation, automatic and explicit rollback, cleanup denial, fresh-governor reuse, panic cleanup, repeated short statements and commit boundaries. Explicitly constructed primary error codes are classification tests, not simulated physical I/O failure. Device-free publisher tests separately cover actual source proof plus catalog mutation. Exact gate results and preservation are recorded in the verification packet after execution.

## SQLite references

The design follows SQLite's [single progress-handler and callback constraints](https://www.sqlite.org/c3ref/progress_handler.html), [interruption and automatic-rollback behavior](https://www.sqlite.org/c3ref/interrupt.html) and [transaction error/rollback semantics](https://www.sqlite.org/lang_transaction.html). The code uses the repository's pinned bundled `rusqlite` runtime; it adds no database dependency or schema migration.
