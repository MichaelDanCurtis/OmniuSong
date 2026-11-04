# Immutable native recording journal, version 1

October 20, 2026. Implements the writer/storage boundary adopted in
[NATIVE-PROTOCOLS.md](NATIVE-PROTOCOLS.md), in `omniusong-recording`.
This is a native recording object format, separate from schema-1 project JSON,
its 30 MiB asset limit and its 4,096 embedded capture-event limit.

The crate implements durable storage, checking, interruption recovery,
immutable sealing, a read-only sealed view and SQLite snapshot export. A live capture callback, transfer rings, 250 ms audio writer
aggregation, 50 ms raw-event aggregation, device alignment and main-project
publication are separate work. The APIs below must run on a writer/control
thread, never an audio callback. Committed/sealed data is not evidence of an
audible performance, microphone permission, measured latency or thirty minutes
of dropout-free recording.

## Public interface

```rust,ignore
Journal::create(path, &TakeDescriptor) -> Result<Journal>
Journal::open(path) -> Result<Journal>
journal.append(&ChunkHeader, payload: &[u8]) -> Result<ChunkReceipt>
journal.read_chunk(stream_id, sequence) -> Result<ChunkRecord>
journal.recovery_report() -> Result<RecoveryReport>
journal.seal(operation_id) -> Result<SealedManifest>
journal.seal_recovered(operation_id) -> Result<SealedManifest>
journal.sealed_manifest() -> Result<Option<SealedManifest>>
journal.descriptor() -> &TakeDescriptor
journal.state() -> Result<TakeState>

SealedJournal::open(path) -> Result<SealedJournal>
sealed.path() -> &Path
sealed.descriptor() -> &TakeDescriptor
sealed.manifest() -> &SealedManifest
sealed.read_chunk(stream_id, sequence) -> Result<ChunkRecord>
sealed.read_ordinal(ordinal) -> Result<ChunkRecord>
sealed.visit_chunks(visitor) -> Result<()>
// visitor: FnMut(u64, &ChunkRecord) -> Result<()>
sealed.export_backup(new_destination) -> Result<BackupReceipt>
```

`TakeDescriptor` has a checked version, core `TakeId`, 1–64 stream descriptors,
and a separate opaque provenance map. Unfamiliar provenance JSON numbers retain
their exact representation through the workspace's arbitrary-precision JSON
support. The descriptor is immutable; its canonical bytes and digest are stored
at creation. A caller issues unique take IDs and separate paths. The crate does
not implement a global registry or reinterpret legacy `assetId` strings.

Each `StreamDescriptor` identifies a stream, segment, original endpoint/source,
connection generation, host-clock domain/epoch and rational timebase, initial
source position, and typed interpretation. Audio declares sample rate, channel
count, audio clock epoch, encoding and channel layout. Supported PCM encodings
are signed 16-bit little endian, signed 24-bit little endian and IEEE-754 f32
little endian, with interleaved or planar layout. Frames/channels/byte count
must agree exactly. A raw-event stream declares `LengthPrefixedCaptureV1` and
does not claim an audio frame position.

This initial API declares the stream/segment set when the take is created. A
device or clock restart needs a separately declared stream/segment or a new take;
it cannot silently change an existing stream's interpretation. New streams
cannot be appended to an already created descriptor in this version.

`ChunkHeader` contains version, take/stream/segment identity, per-stream sequence,
typed `SourceRange`, full interpretation, observed start/end host timestamps,
optional complete clock anchors and content/gap kind. Clock anchors are validated
against their host timebase and audio epoch/rate where applicable. Their complete
values are included in the authenticated header; the caller must issue distinct
anchor IDs for distinct calibrations. Alignment remains reversible metadata.

## Storage ownership and transactions

Every take has a separate SQLite database. `create` exclusively creates a new
file. Creation and `open` hold a nonblocking OS file lock on a canonical sibling
`<journal>.owner.lock` for the lifetime of the `Journal`. A live second owner
receives `RecordingError::Owned` before any interruption transition. Process
death releases the OS lock; merely finding a lock file is not evidence that an
owner is alive. The sidecar is retained and is not an expendable capture file.

On Unix, acquisition rejects database hardlinks (`nlink != 1`) before opening
SQLite, again after locking, and before transitioning to interrupted. This keeps
an unsupported hardlink alias from bypassing canonical-path ownership. Ordinary
symlink aliases resolve to the same canonical lock. External renaming/replacing
the active database or modifying its SQLite tables outside this API is not a
supported writer workflow. Windows uses the same OS-lock API and canonical-path
sidecar; hardlink/alias ownership qualification remains open because the stable
standard-library Windows metadata API does not expose the required link-count
guard. Windows acceptance cannot be inferred from the Mac implementation.

The lock uses the standard library's
[File::try_lock](https://doc.rust-lang.org/std/fs/struct.File.html#method.try_lock).
The database file itself is not whole-file locked, avoiding an unqualified
interaction with SQLite's Windows byte-range locking. No PID-file lease,
timeout-based live-owner takeover or automatic recording restart is used.

The writer requires `journal_mode=DELETE`, `synchronous=FULL`, schema version 1,
and checked transactions. The connection has a 2 MiB page-cache target and memory
mapping disabled. A pending main-project transaction uses a different database
and cannot hold this journal's transaction lock. This does not remove filesystem
contention or establish an fsync deadline.

Database tables are:

| Table | Durable meaning |
| --- | --- |
| `take_meta` | Immutable descriptor/digest and recording/interrupted/sealed state |
| `stream_prefix` | Cached contiguous source ends and next sequence, recorded/gap units |
| `chunks` | Global ordinal, stream, sequence, canonical header, bounded payload and digest |
| `interruptions` | Immutable original interruption report, identity and digest |
| `recovery_index` | Immutable originally verified ordinal/hash snapshot, bound by the interruption receipt |
| `sealed_manifests` | Immutable finalization operation, canonical manifest and digest |

SQL triggers reject in-place updates or deletion of originals, descriptors,
interruption/index receipts and sealed manifests. They are accidental-mutation
guards, not an OS sandbox against an actor replacing the database or rewriting
all receipts. Sealing and opening also verify hashes and cross-record
consistency. A malformed or unsupported journal is retained and rejected.

All native SQLite counters cross explicit checked signed-64-bit conversions.
Negative stored values fail instead of becoming huge unsigned ranges. Clock
ticks remain exact u64 values in canonical JSON and never pass through a signed
SQLite integer column.

## Chunk admission and canonical hashes

Audio payloads are at most **1 MiB**; raw-event payloads are at most **64 KiB**.
Headers are at most **64 KiB**, descriptors/interruption receipts at most
**256 KiB**, and complete sealed manifests at most **512 KiB**. These are separate
native limits, not enlarged legacy import limits. Source end and sequence must
fit checked journal integers. There is no whole-take audio or event-count cap.

`SourceRange::AudioFrames` and `SourceRange::Events` carry a start and strictly
positive count. Sequence begins at zero per stream; the first range begins at
the descriptor's declared source position. Later ranges must begin exactly at
the preceding end. Lost ranges are explicit `ChunkKind::Gap` records with a
typed reason and no payload. They advance known source extent, increase gap
counts, and never fabricate successfully captured silence or MIDI data.

The raw-event encoding is a sequence of 4-byte little-endian lengths followed
by the exact original `omniusong.capture` envelope bytes. Each bounded record is
decoded/validated through the core contract, including source identity,
connection generation, clock domain/epoch and declared timestamp bounds. The
declared event count must match. `encode_event_chunk` helps build one bounded
chunk and preserves the original JSON bytes. Producer-side assembly of a long
SysEx/packet stream and any new fragment codec are not implemented here; this
version stores the bounded capture envelope records the producer supplies.

Canonical metadata is compact UTF-8 JSON serialized from the versioned strict
Rust structs in declaration order; opaque maps use ordered keys. Exact unknown
JSON numbers are retained rather than normalized through f64. This is a defined
OmniuSong encoding, not a claim of conformance to a general JSON canonicalization
standard. Unknown structural fields/versions fail. New semantic layouts need a
new checked version.

The chunk digest is SHA-256 over:

```text
"OmniuSong.recording-chunk.v1\0"
+ header_byte_count as u32 little endian
+ canonical_header_bytes
+ exact_payload_bytes
```

Changing interpretation, clocks, source ranges, gap reason or bytes therefore
changes the digest. A chunk insert and durable-prefix advance share one
transaction. `ChunkReceipt` is returned only after commit, and identifies the
take, stream, sequence, hash and typed latest durable prefix. MIDI-only receipts
contain `SourcePosition::Events`, without invented audio frames.

An exact existing chunk retry verifies its stored header and bytes and returns
`replayed=true`; a conflicting retry is rejected. A failure after commit but
before delivery can therefore be retried without duplicate capture. Interrupted
and sealed takes admit exact retries only within their proven prefix, never new
capture chunks.

## Interruption, bounded recovery and sealing

After acquiring ownership of an unfinished take, `open` verifies its committed
prefix and atomically records `Interrupted` plus an immutable receipt/index.
This is recovery admission, not continuation. It does not delete originals,
repair corrupt bytes or resume recording. A live owner's ordinary reporting call
does not transition its state.

Recovery reads SQLite `length(...)` before allocating descriptor, header,
payload, interruption or manifest buffers. Raw-event and gap payload bounds are
applied before their payload allocation as well. One bounded row is verified at
a time. The reader checks global ordinal continuity, per-stream sequence and
source continuity, declared interpretation/clocks, canonical headers, hashes
and cached durable prefixes. At the first invalid/discontinuous row it retains
the proven global original prefix and reports the problem and unknown tail.
Corrupt or oversized rows never become valid silence. Physical corruption that
prevents reading the descriptor/database is an actionable error, not a claim of
a recovered take.

Normal `seal(operation_id)` validates the complete original sequence and stores
the manifest and sealed state in one transaction. The manifest contains the
complete descriptor, typed stream-prefix/gap summaries, byte/chunk counts and
an ordered index root. The root hashes its domain, descriptor digest, and every
global ordinal plus chunk digest; each chunk digest binds header and content.
The manifest references the existing immutable chunks and never copies PCM into
project JSON.

`seal_recovered(operation_id)` is a separate explicit action available only for
interrupted capture. It includes the original interruption receipt, reports an
unknown tail, and references only the currently verified original prefix. The
retained index is first rehashed in bounded rows against the interruption's
original root; the selected prefix is then rehashed from those original expected
digests and compared to the current bytes. Recomputed replacement chunk hashes
cannot silently contradict the retained original provenance. A shorter prefix
can be selected after further damage only when this original-index proof
matches. Unselected damaged/tail rows and the interruption receipt remain in
the database.

The same finalization operation/mode returns the existing manifest. A conflicting
operation or recovery/normal mode is rejected. Opening a sealed journal verifies
the manifest, descriptor, referenced prefix and interruption proof; a corrupt
sealed row is not accepted as completed capture. Sealed recovery still reports
its unknown original tail.

## Read-only sealed views and owned snapshots

`SealedJournal` opens SQLite with `SQLITE_OPEN_READ_ONLY`, enables connection-local
query-only mode and holds one deferred read transaction for its lifetime. Its
first reads establish the view, check version 1 and rollback-journal mode, and
verify the descriptor, complete selected sealed prefix, manifest and retained
original interruption/index proof. It never calls writer `Journal::open`, creates
an owner sidecar, changes recording state or takes over ownership. Active and
interrupted unsealed journals are rejected without recovery admission. Multiple
sealed readers can coexist with the original live sealed writer owner. Long-lived
read transactions hold SQLite shared locks; they belong on storage threads and
should be dropped when the operation finishes.

The returned descriptor/manifest describe the actual opened view. The source
path is a local path selected by the caller, not an archive path or a capability
grant. Read-only symlink and hardlink aliases are allowed because these readers
do not change writer ownership. Unlike writer acquisition, this API does not
reject a source simply because its link count exceeds one. A source file opened
read-only need not have writable filesystem permissions. A hot rollback journal
requiring write recovery is an error rather than permission to repair the source.
External pathname replacement does not authorize a new identity: the held SQLite
view remains the authority for all subsequent reads/export, and callers bind
`TakeId` plus manifest hash. This is not protection against a hostile actor
rewriting all metadata, receipts or filesystem paths with the same OS authority.

`read_chunk` uses zero-based per-stream sequence; `read_ordinal` uses the
one-based global original ordering. Both restrict access to the sealed selected
prefix. A recovered manifest's unselected tail is never returned as an accepted
chunk, even if a tail header appears parseable. `visit_chunks` visits global
order, rechecks stream/source continuity and the ordered manifest root, and keeps
only one bounded chunk plus the at-most-64 stream summaries in library memory.
The callback may stop with an error. Only a successful complete visit proves all
selected chunks were visited; callers must not publish partial visitation as a
completed ingest. Metadata/payload lengths are checked before allocation by the
same bounded reader as writer recovery. Callers may retain their own callback
copies, which is outside this library's memory bound.

`export_backup` uses the
[SQLite Online Backup API](https://sqlite.org/backup.html), through the existing
rusqlite `backup` feature, from that same held transaction. It never copies the
active source database file with a raw filesystem copy. It moves at most 16
SQLite pages per step (at most 1 MiB for SQLite's maximum page size), with a 2 MiB
page-cache target and mmap disabled. A busy/locked backup fails with explicit
retained staging information; it does not silently retry indefinitely. The
destination is reopened read-only and checked against the source descriptor and
manifest, including selected prefix and retained interruption proof. Known-table
row counts are compared as an additional completeness check.

Backup copies all logical database rows, not just selected playable chunks.
Unselected damaged tail headers/payload bytes, interruption receipts, original
index entries, sealed manifests and any other logical database tables survive.
Those retained bytes do not become playable or count as verified capture. The
export is a consistent SQLite database image; page layout, unused page contents
and file bytes may differ from the original database. Semantic identity is
`TakeId` plus `manifest_sha256`, while `BackupReceipt.sha256` authenticates the
complete **exported file** and `byte_len` counts that file's bytes. File hashing
uses a fixed 64 KiB buffer. Two exports of the same semantic manifest need not
share physical bytes or a file hash.

The destination's parent must already exist and be a trusted local directory.
Export allocates an exclusive UUID staging directory (mode 0700 on Unix), then
an exclusive new SQLite file inside it. It verifies the backup, hashes and fsyncs
the completed file, atomically creates the requested destination with a hardlink
that fails if any name already exists, removes only its successful staging alias,
and fsyncs the parent before returning success. It never overwrites an existing
file or follows an existing destination symlink, including a dangling symlink.
It does not create missing destination parents or accept archive entry names as
trusted paths. Filesystems without atomic hardlinks or directory fsync fail
closed; Windows durable-export qualification remains a real acceptance gate.
Hostile same-user replacement of the trusted staging/parent namespace is outside
this standard-library filesystem contract.

On failure, `RecordingError::Backup` reports `staging_path` (an optional retained
private directory), `destination_path`, `published` and `message`. `published`
means a destination filename was created, not that a project was published. A
failed export retains any partial staging database or complete destination
orphan for inspection; it never deletes originals or automatically cleans up an
uncertain object. Success returns `BackupReceipt { path, byte_len, sha256,
manifest_sha256, take_id }`. A main store can durably ingest this file as an
owned content-addressed object first, then publish its registry/project/history
reference in a separate main-store transaction. It must bind semantic identity
and verify an already-owned object on retry instead of assuming a retry produces
the same physical backup file. Storage failure must not publish a partial
project; retained unreferenced owned objects are legitimate orphans.

## Fault boundaries, tests and remaining gates

The optional `test-hooks` feature enables `set_boundary_hook` and
`clear_boundary_hook`. A hook receives `AfterChunkInsert`, `BeforeChunkCommit`,
`AfterChunkCommit`, `AfterManifestInsert`, `BeforeSealCommit` or `AfterSealCommit`.
Returning an error before commit rolls back the transaction. Returning an error
after commit simulates a lost acknowledgement; committed originals remain and
retry must recognize them. A test child can block in a hook for a real process
termination test. Hooks run on the journal writer thread, never a callback.

Implementation tests cover streamed audio above 30 MiB, more than 4,096 raw
events, exact opaque provenance, restart/interruption/no-resume behavior,
source gaps, format/clock/version/bounds rejection, transaction rollback,
committed lost acknowledgements, corruption/truncation and recovered-prefix
retention, sealed metadata bounds, original SQL immutability, full-u64 host
ticks, signed SQL boundary rejection, rehashed replacement detection and a
simultaneous unrelated SQLite write transaction. Test fixtures use exclusive
UUID directories under ignored `.runtime/rust-migration/recording-tests`.
Independent regression tests belong to `tests/review_regressions.rs`; sealed
reader/snapshot review is reserved to `tests/snapshot_review.rs`.

The coordinator's all-features test log records **16 implementation unit tests
and 10 independent recording test entries passing**. The independent suite
includes a subprocess helper and real termination at all six append/seal
boundaries, ordinary/symlink/hardlink ownership checks, and full/shorter-prefix
replacement detection. These counts describe software tests, not device capture
or an audio-stability result.

Additional snapshot implementation cases cover live sealed ownership with
concurrent readers, active/interrupted rejection without mutation, ordinal/range
and gap/root visitation, streaming backup above 30 MiB, file receipts, exact
unfamiliar numeric metadata, recovered damaged-tail/receipt/index retention,
read-only aliases/permissions, held-view identity after source pathname
replacement, existing/dangling-symlink destinations, and version/tamper/oversize
rejection. Their final coordinated test/build/review gate is recorded separately
by the coordinator; source coverage alone is not a passing test result.

The coordinator owns the final combined build/test results and independent
review evidence. Actual power-loss/fsync behavior, disk-full/stall handling,
writer queue overflow, native device capture,
Windows ownership and 30-minute audio stability remain acceptance gates.
Main-store publication of sealed-take references is deliberately pending the
native musical-object resolver/schema; this crate never adds an opaque take
reference to schema-1 JSON or publishes a partial project track.
