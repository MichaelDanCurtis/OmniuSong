# Owned native take storage and publication, revision 1

October 20, 2026. [`omniusong-store::takes`](../../crates/omniusong-store/src/takes.rs)
resolves the structural references in [NATIVE-TAKES.md](NATIVE-TAKES.md) to owned,
verified originals. Source chunks, immutable interpretation and interruption
proofs follow [NATIVE-RECORDING.md](NATIVE-RECORDING.md). Publication shares the
global ledger in [NATIVE-EDITS.md](NATIVE-EDITS.md); complete retention across
export/restore follows [NATIVE-ARCHIVE.md](NATIVE-ARCHIVE.md).

These are storage-worker APIs. They do not start capture, request microphone
permission, render audio, resample a source, interpret raw gestures as notes or
establish measured latency. Ingestion and publication are separate operations.

## Public interface and identities

```rust,ignore
NativeStore::ingest_take(&mut self, &SealedJournal) -> Result<TakeObject>
NativeStore::registered_take(&self, &TakeId) -> Result<TakeObject>
NativeStore::open_take(&self, &TakeReference) -> Result<SealedJournal>
NativeStore::publish_take(&mut self, &TakePublication) -> Result<EditReceipt>

TakeObject {
    take_id: TakeId,
    manifest_sha256: String,
    object_sha256: String,
    byte_length: u64,
    manifest: SealedManifest,
}

TakePublication {
    operation_id: OperationId,
    project_id: ProjectId,
    expected_revision: u64,
    track_id: TrackId,
    track_name: String,
    clip: TakeClip,
}
TakePublication::validate(&self) -> Result<()>
```

Both structs serialize their declared snake-case fields and reject unknown
fields. Nested musical fields retain their core schema's representation.
`TakePublication` contains identities and a non-destructive view, without an
external source path. Validation checks identities, a publishable signed-SQL
revision boundary, track name, and the core clip structure. Availability and
source interpretation require store admission as well.

Logical identity is `TakeId` plus the recording manifest's domain-separated
`sha256`. `object_sha256` is a plain SHA-256 over every byte of the owned SQLite
snapshot, including retained unselected data. `byte_length` counts that file's
bytes. Logical identity and physical file identity are distinct: SQLite backups
of the same sealed manifest may have different page layouts and file hashes.
A source pathname never becomes a playback authority or a project reference.

## Owned path and immutable registry

The store records its canonical root. Take objects live only at:

```text
<canonical-store-root>/takes/<64-lowercase-hex-file-SHA256>.sqlite3
```

The root and `takes` must be real directories. A redirected/symlinked `takes`
directory fails rather than resolving through it. Object names are constructed
only after validating exactly 64 lowercase hexadecimal characters; a Take ID,
stream ID, external filename or archive entry name is never concatenated as a
storage path. Reading an object requires an ordinary nonsymlink file. Unix also
requires `nlink == 1` and checks device/inode identity around opening and
verification, rejecting a substituted path or external hardlink alias.

SQLite **store schema 3** adds `native_takes` with `take_id` as primary key,
`manifest_sha256`, `object_sha256`, positive signed-SQL `byte_length`, and
`manifest_json`. Update/delete triggers retain existing registrations. The
canonical manifest is at most 512 KiB; SQL checks its stored byte length before
allocating the JSON string. Registry loading rejects negative byte lengths,
invalid digests, malformed/unsupported manifests, noncanonical JSON, and
manifest/descriptor/Take ID disagreements. The manifest retains exact unfamiliar
JSON numbers through the existing arbitrary-precision JSON configuration.

The registry was introduced in store schema 3 and remains unchanged in schema 4,
which adds copy lineage. Store, musical document, journal and archive format
versions are independent; current exports use OSARv2 with strict OSARv1 reading. Migrating the store does not upgrade or attach a
recording to an existing project. Immutable registry triggers guard accidental
mutation; they do not authenticate a database against an actor who can replace
both objects and their expected hashes.

`registered_take` authenticates the owned file before returning its metadata.
`open_take` additionally checks exact manifest identity and stream existence,
then returns the verified read-only sealed journal. Verification opens a sealed
SQLite view, checks the selected original prefix and recovery proof, and streams
the complete owned file through a fixed 64 KiB SHA buffer. It compares the
registered hash and length, including otherwise unselected damaged tail bytes.
One bounded original chunk is read at a time; whole-take audio is never loaded
into project JSON or a Rust buffer.

## Durable ingestion before registration

`ingest_take` accepts an already verified `SealedJournal`, whose held read view is
the source authority. It uses that reader's consistent SQLite Backup export into
a new exclusive UUID stage in the owned directory. This preserves every logical
row, including original chunks, unselected damaged tails, interruption receipts
and the retained original index. It does not raw-copy an active database file.

After the recording exporter has fsynced its staged file and parent, ingestion
atomically creates the SHA-addressed destination with a hardlink that cannot
overwrite an existing name. It removes only its own successful staging alias,
fsyncs the owned directory, and authenticates the resulting ordinary single-link
object. Only then does an immediate main-store transaction insert immutable
registry metadata. A return value acknowledges the registry commit as well as
the completed file durability steps. It does not attach a track to a project.

If the SHA-addressed name already exists, the store authenticates it before
reusing it. An existing symlink, hardlink, damaged file or inconsistent manifest
is an error. A matching existing Take ID and semantic manifest reuses the
already registered object after verifying it, without requiring a new physical
backup to have the same bytes. A different manifest for that Take ID conflicts.
The transaction rechecks concurrent registration under its lock; a losing
equivalent ingest can return the winning verified object while retaining its own
unreferenced object.

A failed file step cannot publish registry/project metadata. A failed registry
insert rolls back that transaction while retaining the durable owned object as
an orphan. A completed ingestion followed by failed project publication retains
the registry and original. No automatic deletion, rollback of originals or
garbage collection is implemented. Partial stages and durable orphans remain
inspectable; cleanup of a successful temporary alias is not deletion of the
owned original.

## Source admission and atomic publication

Only declared schema-2 recording clips create native take dependencies. Legacy
unknown fields named `recording`, `takeId`, `assetId` or `clips` remain opaque.
For each selected stream the resolver requires:

1. Exact registered Take ID and manifest hash, with an existing declared stream.
2. `start >= initial_source_position` and positive half-open range end no greater
   than the selected sealed stream prefix.
3. Matching range units: audio frames for audio, event ordinals for raw events.
4. Exact source PCM/arrangement sample-rate equality for audio, with no implicit
   resampling. Event ordinals are never added as an audio duration; their source
   host clocks remain in the original independently of signed arrangement placement.

An extent containing an explicit source gap remains an extent with a gap. It
does not establish successfully captured silence or uninterrupted performance.
Recovered manifests retain their unknown-tail qualification; clips can select
only their proven original prefix.

Publication serializes this deterministic global-ledger request:

```text
{"kind":"take_publication","version":1,"intent":<TakePublication>}
```

Struct declaration order and the strict nested types determine canonical
serialization. The existing operation request byte ceiling applies. A matching
committed retry is checked before current revision admission and returns the
original committed result with `replayed = true`, even after newer edits, branch
truncation or external source loss. The store authenticates that historical
result's owned originals before returning it. Reuse of the operation ID for
different content, another project or a regular edit is rejected.

For a new operation, one immediate main-store transaction checks the expected
revision, explicitly upgrades musical schema 1 to 2 if needed, and appends the
typed clip. It creates a recording track when its ID is absent, or appends to an
unlocked existing recording track without renaming it. It never reinterprets a
notes/audio track or rewrites opaque legacy fields. The shared save helper then
checks the result's references and owned originals before project/history SQL
mutation. Project revision, history/cursor and global operation receipt commit
together. Failure, including a final receipt insertion failure, leaves the prior
project/history/receipt state visible and retains the ingested original.

## Strict result checks and cost

Every save, undo, redo, regular admitted edit and take publication authenticates
every distinct take referenced by its **resulting** document before commit.
Unchanged Take IDs are checked as well: a cached document cannot publish another
revision retaining an original that has become missing or corrupt. An edit that
entirely removes a damaged reference may succeed when all remaining result
references are valid. Historical receipt replay separately authenticates its
original result.

Clips are grouped by Take ID. Each result check loads one bounded manifest at a
time, validates every clip's stream/range, and computes one complete file SHA per
distinct referenced take. Multiple clips on the same original do not cause
repeated full-file hashing in that preflight. Selected chunk/proof verification
also reads source data, so this is linear storage I/O, not a constant-time
metadata edit. Run these APIs on a storage worker, never an audio callback or
latency-sensitive UI path. A verified cache with explicit immutable object
identity, invalidation and external-corruption rules is future work; no degraded
save or unchecked cached-source policy is implied.

## Verification scope and platform limits

### Explicit sealed-capture attachment

`NativeStore::attach_capture(journal_path, publication)` connects the engine's explicit Finish result to existing ingestion and project publication. The background session service exposes this as `AttachCapture`. It checks the authenticated durable operation receipt before reading a stage path. An already registered exact original can be reused after the stage disappears; otherwise the sealed stage's take/manifest identity must match before durable ingestion. Project revision conflict leaves the ingested original available. Retrying a completed service action returns the latest saved project, preserving edits made after the original receipt. The same operation identity is retained across an explicit retry; an unknown or unsealed tail is never accepted as a normal finished recording.

Store and actual session-worker regressions cover lost acknowledgment, disappeared stage, later edits, conflict and original preservation. The [software-bus qualification](../verification/rust-native-audio-software-bus.json) captures synthetic PCM through the real journal/publication/archive path. It opens no native input and establishes no physical capture or latency result. General discovery and user selection of unassigned capture stages remains pending.

Focused implementation fixtures cover semantic retries with different physical
source layouts, valid raw-event windows at signed timeline limits, registry
metadata read caps and negative SQL lengths. Independent take publication tests
cover source/range/rate rejection, provenance preservation, locks, concurrent
global receipts, rollback, orphan retention, history/branching and owned-file
corruption/alias guards. The coordinator records the combined test, formatting,
Clippy and archive integration results; source fixtures alone are not a passing
gate or a real recording result.

Durable file publication requires atomic local hardlinks and directory fsync;
unsupported filesystems fail closed. Unix single-link and device/inode guards
are implemented. Equivalent Windows alias/directory-durability qualification is
pending and cannot be inferred from Mac tests. Trusted store/parent directories
are a filesystem precondition; this is not a sandbox against hostile same-user
namespace replacement or direct writes that bypass SQLite locks. Power-loss,
disk-full/stall behavior, writer scheduling and real device/capture integration
remain separate acceptance gates.
