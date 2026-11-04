# Atomic original-performance projection publication

This contract extends the [held binding contract](NATIVE-PERFORMANCE-PROJECTION-BINDING.md). It selectively opens source-aware store publication, current load, ordinary edits and history. The legacy guards remain in archive, metadata-reader, listing and attachment consumers. This is a Rust library gate; it does not enable a native conversion control or qualify an installed application.

## Portable intention and compact commitment

`ProjectionPublication` version 1 is strict snake-case JSON, with no unknown fields and at most 65,536 encoded bytes. It contains the project ID, expected revision, projection ID, exact interpretation/source request, expected review SHA-256, sorted new destination-lane choices, timeline policy and explicit schema-upgrade consent. It contains no caller-supplied notes, retained review blob, document, file path or filesystem-origin claim. Lane names are bounded before Unicode traversal. Revisions must permit one adjacent signed SQLite revision.

The operation ID is `os-perf-proj:` followed by lowercase SHA-256 of:

1. The exact domain bytes `OmniuSong.performance-projection-operation.v1\0`.
2. The little-endian u64 byte length and exact bytes of the project ID.
3. The little-endian u64 byte length and exact bytes of the projection ID.

This deterministic key permits an indexed receipt lookup without a history scan or catalog migration. Fresh generic edit, take-attachment and archive-copy operations cannot claim this namespace. Exact preexisting ordinary receipts remain replayable before the fresh-operation restriction; a historical arbitrary identifier does not acquire projection authority.

`NativeStore::publish_performance_projection(intent, budget)` returns a strict `ProjectionCommitReceipt` of at most 8,192 encoded bytes. The receipt contains adjacent revisions, exact request/result/review hashes, the original audio take reference and lane/note counts. It returns no playable project. `projection_publication_receipt` observes the same exact catalog commitment without opening original files.

Recorded consistency and original-source authority are separate. A coherent forged catalog can describe false music and satisfy the compact receipt's consistency checks. A receipt is not a cryptographic attestation that the source was honestly converted. Current source-aware load, editing and history independently rederive every retained binding from held originals. The false-pitch oracle deliberately demonstrates this distinction.

## Atomic write and retry boundary

The typed publisher holds one immediate catalog transaction. Its order is:

1. Validate the complete bounded intention and look up an exact prior commitment. A matching prior result returns its compact recorded fact before caller cancellation or original-file acquisition. A different request under the same operation ID refuses.
2. Hold the retained-file coordination lease, check cancellation, read the bounded current project and compare exact revision and tempo bits.
3. Resolve registered originals and authenticate each distinct performance take once under the same sticky cumulative governor. Validate existing bindings and original/current clip geometry.
4. Build the new immutable binding from the actual original and expected review digest. Construct the complete candidate with the core constructor. Append exactly one binding and every initially muted ordinary note lane, preserving all previous document fields and originals. Explicit schema consent is required for musical schema 1/2 to 3; an old opaque same-named field cannot be overwritten.
5. Write the project, one before/after edit, cursor and exact operation result in the same transaction. Verify the project/history/cursor postimages and exact canonical receipt while rollback remains possible.
6. Verify all referenced legacy assets after SQL mutations, reload and compare every registered `TakeObject`, recheck held file identities and paths, then check the same governor immediately before commit.
7. Commit and return the already constructed receipt. There is no fallible validation or cancellation checkpoint after successful commit.

A failure before commit rolls back catalog mutations. A commit error can leave completion uncertain; reconcile by the exact saved intention and operation ID. Never regenerate an ID, reinstall an old snapshot or infer rollback from a late error. Compact replay remains available when originals or the current project are unavailable and never overwrites a newer branch.

The retained registration comparison is necessary even with immutable UPDATE/DELETE triggers: SQLite `INSERT OR REPLACE` from another trigger can bypass a DELETE trigger when recursive triggers are disabled. The final comparison rejects substitution without opening the substituted path. Legacy-asset verification similarly follows every mutating statement so a trigger cannot commit a corrupted referenced asset.

## Held-source and metadata budgets

`RegisteredProjectionSources` groups exact schema-owned clip and binding references by registered take. It resolves ordinary owned paths, bounded manifests and exact descriptor/manifest/physical-file identity. Performance authentication retains its reader, original proof, file stamps and shared governor through the transaction. Nonperformance takes retain their existing bounded owned-object verification. No caller-provided inspection or stored review digest is accepted as a source proof, and a damaged owned object does not fall back to a staged copy.

Work, rows, records, payloads and interpretation passes remain cumulative across references, bindings and history documents. File-byte accounting is the maximum inspected file size, not an authentication-call counter. The governor-identity check does not mint or reset any allowance.

Dispatch uses a thin `IgnoredAny` ownership probe rather than materializing arbitrary JSON trees. Source-aware project/history/operation bytes are length-checked and prepaid before full decoding; manifest and legacy-asset bytes are prepaid before materialization and hashing. Exact metadata and file postimages are checked before the transaction ends. Actual main-catalog SQLite I/O/corruption/not-a-database failures retain their domain even if cancellation becomes sticky later; source-file SQLite failures remain source errors.

Cancellation of the reusable live catalog is currently checked **between SQL statements**. Its SQL VM does not have a scoped progress hook, so a single statement or preexisting trigger can run until that statement returns. Source-reader SQL retains its existing bounded progress policy. A rollback-safe scoped live-catalog VM/cancellation design and responsiveness qualification are required before native job/control admission; installing a stale private-reader hook on the reusable connection is not an acceptable shortcut.

## Active edits and durable history

Current `NativeStore::load` and `load_with_budget` dispatch nonempty owned schema-3 documents through source authentication. Ordinary projects keep their existing legacy loading limits and behavior.

Generic `save` and supported `apply_edit` mutations may change the current editable notes, track names/patches, tempo and geometry, including deletion of current notes or lanes. They must preserve an admitted projection registry exactly. They cannot insert, remove, reorder or rewrite original ownership, downgrade the owned schema or promote an old opaque field. Generic Create still refuses a nonempty registry. Original identities remain reserved after current-note deletion.

Undo and Redo activate authenticated targets and preserve monotonic current revisions. Both sides of a history transition are bounded and checked. Ordinary transitions retain exactly the same registry. A publication transition must be exactly one appended binding with its matching typed operation and complete original candidate, reconstructed from the recorded before snapshot. The after-bindings are rederived once; exact registry prefix equality covers the before-bindings without another identical original interpretation. Current content must agree with the selected history side before navigation.

Generic historical edit replay containing projections requires current original authentication. This differs from the dedicated compact projection receipt, which can report recorded commitment while originals are absent.

## Integration gates that remain closed

- The SQLite catalog remains schema 6. No default studio migration, current-project conversion or installed-bundle replacement is part of this slice.
- `NativeReader`, guarded list metadata, OSAR export/import/copy and unmigrated take/source-attachment consumers still refuse nonempty projection authority. A source-aware project may therefore be unavailable through those consumers until their explicit integration. Do not remove their guards globally.
- Complete OSAR current/history/result/copy-proof semantic validation and inert OSRE closure remain required. A future copied project must derive its destination operation key from its destination project identity, preserve original binding identity and explicitly map owned operation lineage. An origin path/hash cannot be rewritten as if it authenticated the copy.
- Durable session admission must save the exact intention, canonical origin/root, full UI context and worker epoch before enabling confirmation or restart retry. The portable store DTO deliberately omits those machine-specific claims.
- Native preview, confirmation, selection/draft safety, current versus historical results, editing, Undo/Redo, archives, restart and original-versus-derived audition need an exact-binary walkthrough after those prerequisites.
- This conversion is procedural interpretation of a recorded performance, not AI inference or complete expressive MIDI editing. MIDI 2 project data, UMP, native transport, MIDI-CI, Clip Files and MPE retain their separate acceptance gates. Physical controllers, listening, sustained recording, model roles, plugins, installers and actual Windows qualification remain required by the full goal.

See the [publication verification packet](../verification/rust-native-projection-publication-tests.json), [independent source review](../reviews/RUST-PROJECTION-PUBLICATION-REVIEW.md) and [implementation plan](../IMPLEMENTATION-PLAN.md) for executed evidence and the next integration work.
