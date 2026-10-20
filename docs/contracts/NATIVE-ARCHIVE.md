# Native project archives, revision 2

The native archive is one portable SQLite file containing a complete retained project closure. [`omniusong-store::archive`](../../crates/omniusong-store/src/archive.rs) implements export and identity-preserving restore; sealed native originals use the [native take references](NATIVE-TAKES.md) and recording snapshot APIs. Archive version, musical schema, main-store schema and recording journal version are independent.

## Public operations and current restore mode

`NativeStore::export_archive(project_id, destination) -> Result<ArchiveReceipt>` exports an existing native project to a new filename. The receipt includes `path`, `project_id`, complete-file `sha256`, `byte_length`, `history_rows`, `operation_rows`, `asset_count`, and `take_count`.

`NativeStore::restore_archive(path) -> Result<ProjectDocument>` restores the original project, history and operation identities. It permits unrelated destination projects. An existing project ID, retained rows for that project ID, or any global operation-ID collision rejects the restore. It does not replace or merge an existing project. Shared assets or takes are reused only after exact content/metadata and owned-file verification; damaged or differing shared objects reject.

`inspect_archive` and `restore_archive_as_copy` provide hash-bound inspection and a new session with fresh project/live-operation identities. The [copy and lineage contract](NATIVE-ARCHIVE-COPY.md) defines their typed transformations, durable retry identity and complete immutable ancestry. Unknown metadata and original import bytes remain opaque. The [background session service](NATIVE-SESSION-SERVICE.md) owns these operations for the native interface.

All archive work runs on a storage thread. It performs blocking SQLite/file I/O, allocation, hashing and process-local validation; it must not run on audio callbacks or a UI paint/input callback. The source's consistent read transaction may delay main-project writers under rollback journal mode. Independently owned recording journals remain separate from the main-project transaction.

## Complete closure

Export includes:

1. The current project JSON, revision and edit cursor.
2. Both before/after project snapshots for every retained history row, including redo rows beyond the current cursor.
3. Every retained operation request and result snapshot for this project, including results whose redo branch was subsequently truncated.
4. The exact original import JSON bytes and their source digest.
5. Every declared legacy asset and native take referenced by any of those snapshots or the original import.
6. Complete registry metadata and full sealed SQLite object bytes for each referenced native take, including retained interruption receipts, recovery indices and rejected/unselected tail rows.

Source traversal uses a read-only SQLite connection and one explicit read transaction. History and operation rows are visited incrementally. It does not materialize an entire history/result set in memory. Legacy metadata names are not searched recursively; only `ProjectDocument`'s declared reference paths create dependencies. An opaque `assetId`, `takeId` or `recording` field does not cause an inferred read.

History sequence is an edit-cursor coordinate, not a project revision. Export/restore validates sequence continuity, cursor bounds, project identities and each before/after revision transition. Undo and redo can raise the current revision without adding history rows; retained operation results can outlive the corresponding redo history. The archive does not require current JSON bytes to equal a historical after snapshot.

Operation requests must be supported typed `ProjectEditIntent` records (`Rename`, `SetTempo`, `UpdateSession`, `Undo`, `Redo`) or the canonical `{kind:"take_publication",version:1,intent:TakePublication}` wrapper. Other operations reject export/restore rather than disappearing. Ordinary intents are recognized by their required `operation_id` field first, so their permitted opaque `kind`, `version` and `intent` extensions retain their original meaning. Request/result hashes, canonical typed request bytes, operation/project IDs and previous/committed revisions are checked. Every immutable copy-proof result is also a source-retention root, even after ordinary history and operation results stop referring to its originals.

## File format and integrity

New exports use SQLite `application_id = 0x4f534152` (`OSAR`) and `user_version = 2`, with rollback journal mode. Format 2 adds the four [copy-lineage tables](NATIVE-ARCHIVE-COPY.md) to the original eight tables below. Strict format-1 archives remain readable with exactly their original eight-table schema and index behavior. Table definitions live in `archive.rs::TABLES` and `archive_copy.rs::LINEAGE_TABLES`. Restore admits the supported definitions and SQLite's corresponding automatic primary-key indexes only. Extra tables, views, triggers, user indexes, virtual tables and modified definitions reject. Read connections use query-only mode, disabled trusted schema, a bounded cache and no mmap.

| Table | Retained content |
| --- | --- |
| `archive_meta` | Singleton project identity, declared row/byte totals and ordered metadata index digest |
| `projects` | One exact payload, revision, cursor and payload digest |
| `edits` | Ordered before/after snapshots with individual digests |
| `operations` | Exact canonical requests, retained result snapshots, IDs, revisions and their existing digests |
| `imports` | Exact source JSON BLOB and source digest |
| `assets` | Declared legacy content-addressed assets |
| `native_takes` | Take identity, sealed manifest digest, physical object digest/length and exact canonical manifest JSON |
| `take_chunks` | Complete physical SQLite object bytes in ordered, bounded chunks |

`take_chunks` has `object_sha256`, zero-based `sequence`, `byte_length`, `sha256` and `payload` fields. Chunks are exactly 1 MiB except the final nonempty remainder. Restore checks sequence, declared/actual size, per-chunk SHA-256, total length and complete-object SHA-256 before opening the staged object as a `SealedJournal`. The full-file digest is distinct from the logical, domain-separated sealed-manifest digest.

The metadata index hashes the domain `OmniuSong.project-archive.v1\0`, then ordered project, edit, operation and import records; format 2 next includes every lineage proof, mapping, ownership association and live admission; asset and take records follow. Every tag/field has an unsigned little-endian 64-bit byte-length prefix; each record includes its field count. Objects are ordered by validated identity; history uses sequence. Asset records bind ID, length and content digest; take records bind take/manifest/object identities, length and canonical manifest JSON. Logical totals are checked against actual visited records. Missing or unrelated rows/chunks reject.

SQLite integrity, per-record digests and the closure index detect corruption and inconsistent/incomplete archives. They are not a digital signature or authentication against someone rewriting all content and digests. Identity-preserving restore has no expected-digest parameter. Copy restore requires the complete-file fingerprint from inspection and checks its private acquired snapshot against that fingerprint before publication.

## Explicit bounds

| Boundary | Initial limit |
| --- | ---: |
| Project/history/operation-result row | Musical schema's own ceiling: 1,900,000 bytes for schema 1; 8 MiB for schema 2 |
| Operation request | 65,536 bytes |
| Original import JSON | 1,900,000 bytes |
| Registered manifest JSON | 512 KiB |
| Legacy asset BLOB | 30 MiB per asset |
| Take-file transfer chunk | 1 MiB |
| Retained edit rows | 10,000 |
| Retained operation rows | 100,000 |
| Legacy assets / native takes | 4,096 of each |
| Metadata accounting | 256 MiB |
| Aggregate legacy/take payload | 1 TiB |
| Physical archive file | 2 TiB, including SQLite overhead |

Metadata accounting includes JSON/import/manifest bytes and bounded record keys, digests and integer fields; each transfer chunk accounts for 144 bytes of identity/checksum/sequence/length metadata. SQLite page/index overhead is governed by the separate file ceiling. These are chosen resource bounds, not measured capacity or performance claims. Exceeding a limit is an explicit failure, never truncation or dropping history.

SQL queries use `octet_length` and `CASE` to reject oversized TEXT/BLOB fields before materializing them in Rust. Object payloads use one bounded buffer; large recordings are never collected into a whole-file `Vec`. The legacy asset path deliberately permits one bounded asset BLOB at a time, and exact shared-asset comparison can hold two such BLOBs. These asset limits do not impose the legacy 30 MiB whole-take ceiling on native recordings.

## Export publication

Export creates a UUID-owned private staging directory beside the requested destination, reserves a new SQLite file and writes the complete archive there. It validates references and object identities before committing and closing the staging database, then syncs the file. An atomic create-if-absent hard link publishes the final name; an existing file or symlink is never overwritten. The owned staging link is removed after publication and the parent directory is synced on Unix.

Failure retains staging artifacts and reports their path, requested destination and whether the final archive name was already published. A failure after final-name publication may leave a complete archive requiring inspection; it is not silently deleted or overwritten on retry. Source projects, originals and sealed object files are read-only inputs throughout.

## Restore staging and publication

Restore first opens the archive read-only, verifies application/version/schema/integrity and checks declared counts/budgets. It validates every metadata snapshot/request and reconstructs take objects into a new UUID-owned staging directory under the destination store. Every object is fsynced, checked against its full-file digest and opened through the sealed-journal verifier. The closure index, manifests, source stream units/ranges, sample-rate policy and legacy asset digests must all pass before destination publication begins.

Only then does one immediate main-store transaction:

1. Check project and global operation-ID conflicts.
2. Verify/reuse exact existing assets and registered takes; insert new registry rows without overwriting.
3. Publish missing verified take object files with create-if-absent hard links. Remove only their owned staging links so owned-object single-link checks remain true; sync files/directories.
4. Insert the exact project/cursor, retained history, operation receipts and original import bytes.
5. Recheck all current/history/result references and commit.

An SQL error rolls back project/history/operation/asset/registry rows together. Physical files published before a failed transaction remain content-addressed orphans and staging diagnostics are retained. They cannot masquerade as a committed project; an explicit retry may reuse an exact verified orphan. Existing objects are never replaced to repair a damaged destination.

Restore errors identify staging, destination root, published-object count and whether the database commit occurred. `restore.json` in owned staging identifies the source archive, project, destination and restore mode. If cleanup fails after commit, the error reports `committed=true`; callers must inspect the committed project rather than blindly repeating an identity-preserving restore. Successful cleanup removes only the UUID-owned staging directory.

No archive-supplied pathname or original filename is extracted. Destination object paths are derived only from checked physical SHA-256 values in the owned `takes` directory. Symlink/alias rules are enforced by the take-object helper. This prevents format-driven path traversal; it is not a defense against a hostile same-user process replacing trusted directories concurrently. Directory-sync and filesystem behavior still need Windows qualification.

## Evidence required

Owned tests cover complete legacy import/asset/history/operation round trips, exact source bytes, unknown intent extensions, no-overwrite publication, unrelated destination projects, project/global-operation collisions, final SQL rollback, retained diagnostics, malformed format/schema/hash/count/budget, truncation and unsupported retained operations. Independent tests and the coordinator's synthetic long-take qualification cover physical take reconstruction, recovered bad-tail preservation and references retained only by old operation results.

Record actual test and Clippy results after the coordinated gate. An archive test, synthetic long take or successful sealed-object copy does not prove elapsed 30-minute device recording, audio latency, plugin behavior or Windows acceptance. Copy-restore and native UI evidence have their own receipts.

Observed October 20, 2026 on the Mac coordinator host: all five owned archive tests and eight independent archive regressions passed in `.runtime/rust-migration/native-takes-store-tests.log`. The [synthetic archive receipt](../verification/rust-native-archive-roundtrip.json) records 33 MiB of PCM, 5,120 raw events and 113 journal chunks retained solely through operation receipts after redo truncation, with exact restored payloads and unchanged source-file bytes. The coordinator records the complete workspace/Clippy disposition separately.
