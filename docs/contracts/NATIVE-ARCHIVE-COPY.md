# Native archive copy and lineage contract

Status: implemented. The coordinator’s fresh `session-copy-workspace-tests-final.log` passes all five owner copy regressions and all ten independent archive-copy review cases. The [archive-copy qualification](../verification/rust-native-archive-copy.json) also verifies an existing OSAR version 1 archive through copy, version 2 export, identity restore and copy-of-copy, retaining 36,846,248 payload bytes and exact ancestor proof bytes. The coordinator owns service/UI and complete workspace release gates. This contract covers native persistence; it makes no device capture, audible playback, latency or Windows qualification claim.

## Public admission and inspection

`NativeStore::inspect_archive(&self, path: &Path) -> Result<ArchiveInspection>` returns:

| Field | Type | Meaning |
|---|---|---|
| `sha256` | `String` | Lowercase SHA-256 of the complete archive file |
| `byte_length` | `u64` | Complete file length |
| `project_id`, `name` | `String` | Current archived project identity and name |
| `track_count` | `usize` | Number of current tracks |
| `history_rows`, `operation_rows`, `asset_count`, `take_count` | `u64` | Archive receipt counts |

Inspection streams the input into a newly reserved private staging file while hashing. Its SQLite summary therefore describes the same bytes as its fingerprint, even if the acquisition path is subsequently replaced. It checks the application, supported format, strict schema, advertised bounds/counts, current document and snapshot checksums. It does **not** certify all operation semantics, lineage, reference closure, manifests or take payloads. Restore performs those checks. Successful inspection removes its owned staging directory; failed inspection identifies retained staging in its error.

`NativeStore::restore_archive_as_copy(&mut self, path: &Path, intent: &RestoreCopyIntent) -> Result<RestoreCopyReceipt>` accepts a typed intent with `operation_id: OperationId` and `expected_archive_sha256: String`. The identifier follows the core stable ASCII identity rules; the fingerprint must be exactly 64 lowercase hexadecimal characters. Unknown intent fields are rejected by serde.

The result contains `operation_id: OperationId`, `source_project_id: String`, `source_archive_sha256: String`, `document: ProjectDocument` and `replayed: bool`. `document.id()` is the new project to open. A successful new copy returns `replayed=false`. Repeating the same operation ID and archive fingerprint returns the **original copied document**, with `replayed=true`, even if that project has since been edited or the archive file is gone. It does not replace the current document or revert later edits. Reusing the ID for another fingerprint, or an ID already owned by a project edit, fails.

The acquisition path appears only in a local failure diagnostic. It is not part of durable intent identity and is not necessary for retry. A retry validates its immutable receipt, ancestry, mappings, retained operations and source objects before returning. Corrupted retained operations or take objects cause an error even when the current document no longer refers to them.

## Typed copy transformation

A new copy gets a fresh project UUID and a fresh global operation UUID for every retained project operation. Current document, both sides of every retained history row, and every operation-result snapshot receive only a new root `id`. Typed project-edit requests receive new `project_id` and `operation_id`; typed take-publication requests receive the same two changes inside their owned intent. Expected revisions, committed revisions, history sequences and the cursor are preserved. Undo/redo semantics remain valid even though history sequence and project revision differ.

Supported archived requests are the core `ProjectEditIntent` operations `Rename`, `SetTempo`, `UpdateSession`, `Undo` and `Redo`, and the version 1 `take_publication` wrapper. Other operations fail explicitly. The generic intent discriminator is its owned `operation_id`, so extensions named `kind`, `version` and `intent` remain opaque; they cannot take over wrapper dispatch.

Track, note, clip, stream and take IDs are retained. All unrelated root and nested metadata is retained as JSON values, including arbitrary-precision numbers, UUID-looking strings and source provenance. Original `imports.original_json` bytes and their source digest remain exact. Import receipt ownership changes to the copied project; the imported source document inside those bytes does not change. There is no recursive string replacement or asset inference from opaque fields.

The closure includes the current document, both sides of retained history, every retained operation result including results whose redo rows were truncated, exact original imports, and every copy-proof result document. Legacy assets and full sealed take files retain their original bytes. This includes a recovered journal's retained bad tails and indexes, not just its selected playable prefix. Existing shared assets, take registrations and proof objects may be reused only after exact metadata/content agreement. Existing projects and immutable objects are never overwritten.

## Immutable lineage and live admission

Native store schema 4 adds these four tables. Their exact table definitions are centralized in `archive_copy::LINEAGE_TABLES`; the main store adds immutable update/delete triggers and cross-kind global operation ID guards.

| Table | Identity and contents |
|---|---|
| `archive_copy_proofs` | `proof_sha256` primary key; exact canonical `receipt_json` |
| `archive_copy_proof_operations` | Primary key `(proof_sha256, source_operation_id)`; unique `(proof_sha256, destination_operation_id)`; source/destination operation IDs plus four request/result SHA-256 values |
| `archive_project_lineage` | Primary key `(project_id, proof_sha256)`; explicit ownership of the complete proof closure |
| `archive_copies` | Live admission: `operation_id` primary key, unique `project_id`, unique `proof_sha256` |

Proof JSON is a strict version 1 record containing:

- `version`, `operation_id`;
- `source_archive_sha256`, `source_archive_byte_length`, `source_project_id`, `destination_project_id`;
- `mapping_count`, `mapping_sha256`;
- `ancestors`: the sorted, unique, explicit list of all ancestor proof hashes;
- `result`: the copied project as a nested JSON value, and `result_sha256`, the hash of `ProjectDocument::to_json()` for that result.

The proof hash is SHA-256 of its exact canonical compact JSON. The mapping hash begins with the bytes `OmniuSong.copy-mapping.v1\0`, then visits mappings in ascending source-operation-ID order. Each of the six fields—source ID, destination ID, source request digest, source result digest, destination request digest, destination result digest—is UTF-8 prefixed by its unsigned 64-bit little-endian byte length. All six fields are included, so changing either side of a mapping breaks its receipt.

An active proof must target its owning project and match its live admission ID. Its ancestors must equal every other owned proof. Every ancestor must exist, exclude itself, and contain all ancestors of its own ancestors. Each non-root edge must extend the unique prior edge whose destination project is its source project; its complete ancestor set must equal that prior edge plus the prior edge's ancestors. Each previously copied operation must occur in the new edge's mapping with matching source request/result digests. Cycles, disconnected evidence, missing mappings and extraneous archive lineage rows fail validation.

The active proof's destination operation IDs and digests are checked against the live retained operation rows. Later operations may be added to that project and need not appear in its original copy map. Historical proofs retain their original IDs and mapping bytes; their IDs are evidence, not destination ledger admissions. Copying a copy carries all prior proofs unchanged and creates one new edge with a new live admission. That distinction prevents ancestor IDs from colliding with an existing original project while retaining their provenance.

SHA-256 and SQLite immutability guards provide integrity and application-level immutability, not an external signature or authenticated claim about an archive's author.

## Archive format evolution

OSAR uses `application_id = 0x4f534152`, independently of the native store schema number and musical document schema. New exports use **archive format 2**. Its exact schema is the original eight archive tables plus the four lineage tables above, without application triggers. Its metadata index includes every proof, operation mapping, ownership association and live admission, before object records; all their bytes consume the shared metadata budget.

Format 1 remains readable with its original exact eight-table schema and original index algorithm. It may not contain lineage tables. Format 2 requires all twelve tables even when no lineage exists. Unknown versions, extra tables, triggers, views and changed table definitions fail. A version 1 archive copied into a native store receives its first lineage edge; subsequent export uses version 2. Version 1 has not silently acquired new fields or tables.

Identity-preserving `restore_archive` remains available: it restores a format 2 project's live copy admission and complete proof closure, preserving the ability to retry its original copy operation. It rejects project and global live operation ID collisions. Restore-as-copy preserves the source archive's admission only as historical proof evidence and creates a fresh live admission for the new copy. Historical proof operation IDs are never entered into the global live operation namespace.

## Transaction, concurrency and failure behavior

All work runs on a storage worker, never an audio callback. The implementation:

1. Validates the intent and attempts a consistent read-transaction replay before reading the external path.
2. Reserves a private UUID staging directory, writes a diagnostic, streams the external archive into an exclusive staging file, and checks the expected complete-file digest and length.
3. Opens only that staged archive read-only. It validates schema, counts, documents, requests, snapshot/index digests, full lineage, complete reference closure and every reconstructed sealed take file before publication.
4. Starts one immediate destination transaction, repeats admission to handle concurrent retries, allocates fresh identities and installs exact shared/new objects without overwriting.
5. Inserts remapped current/history/operation/import rows, inherited proof ownership and the new proof/map/admission. It validates final copy budgets, mappings and source objects, then commits once.
6. Removes only its own staging directory after success.

Two connections submitting the same intent serialize at admission: one publishes; the other returns the original receipt. A final SQL failure rolls back every project/history/operation/lineage/registration row. Content-addressed take files installed before the transaction failed may remain as identifiable orphans, and the owned staging directory remains for diagnosis. Error text includes staging, published-object count and commit status. Cleanup errors after commit report `committed=true`; retry recovers the durable receipt without the archive. There is no automatic broad directory cleanup or replacement.

## Explicit bounds

These are initial engineering limits, not measured performance claims:

| Resource | Limit |
|---|---:|
| Complete archive input file | 2 TiB |
| Combined asset/take payload | 1 TiB |
| Combined archive metadata, lineage and chunk accounting | 256 MiB |
| Current or historical native document | Schema 2: 8 MiB; schema 1 retains its existing limit |
| Proof JSON row | 8 MiB + 32 KiB |
| Proofs owned by one project | 256 |
| Operation mappings in one proof | 100,000 |
| Aggregate mappings across owned proofs | 1,000,000 |
| Retained edit-history rows | 10,000 |
| Retained live project operations | 100,000 |
| Take chunk | 1 MiB |
| File acquisition/hash buffer | 64 KiB |

SQL byte-length guards precede variable-sized row materialization. History, operations, mapping rows and take chunks stream sequentially; they are not collected as one in-memory history. Bounded proof identities/ancestry are retained to validate the graph. Temporary mapping rows use a private SQLite TEMP table. Copy and inspection may require substantial temporary disk space because the complete input is staged; take reconstruction additionally requires its full physical object size. A limit failure is explicit and cannot silently drop history, receipts, lineage or original bytes.

## Verification scope

Owner regressions exercise typed-only rewriting with opaque IDs and exact numbers, `UpdateSession` compatibility, mapped operation retry, missing-file retry after later edits, copy-of-copy proof preservation, identity restore of copied admissions, strict version 1 compatibility, changed fingerprints, global ID collisions, final SQL rollback, source-file preservation and concurrent same-intent publication. Independent review tests live in `tests/archive_copy_review.rs`; the coordinator records actual integrated results separately. The current qualification uses synthetic stored content and records unchanged source files, original project, mapped historical roots and retry behavior. Real audio/MIDI recording, plugin hosting and device qualification remain separate work.
