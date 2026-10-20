# Native musical take references, revision 2

Musical schema 2 adds explicit references to sealed native recordings. It does not put long audio or raw event streams in project JSON. The types and document operations are in [`omniusong-core::native`](../../crates/omniusong-core/src/native.rs); immutable recording storage and source clocks remain governed by [NATIVE-PROTOCOLS.md](NATIVE-PROTOCOLS.md) and the recording journal.

This is a musical document version, separate from SQLite store schema versions, recording journal/manifest versions, and IPC protocol versions.

## Explicit upgrade and collision rules

New `ProjectDocument` instances remain schema 1. `upgrade_native(&self) -> Result<ProjectDocument>` accepts only schema 1 and returns an independent value with only `schemaVersion` changed to 2. It preserves project identity, revision, history, import receipts, original note/clip snapshots, provenance, unknown metadata and exact unfamiliar JSON numbers. Repeating the upgrade on schema 2 returns an error. The store owns any publication revision and history transaction.

Only the combination **`schemaVersion: 2` and track `kind: "recording"`** declares the required `recording` payload. A schema-1 project cannot contain this new kind. Existing `notes` and `audio` tracks preserve their existing semantics under both document versions. Their unknown fields named `recording`, `takeId`, `clips`, or similar remain opaque and are never projected as native references. Root/history/provider metadata is not searched recursively for matching field names.

A recording track owns the common `id`, `name`, `kind`, `gain`, `muted`, `locked`, optional `armed`, and required `recording` fields. Other fields on that new kind remain opaque. In particular, `assetId`, `sourceAssetId`, `audio`, `notes`, and `provenance.input.assetId` on a recording track do not create legacy asset dependencies or editable-note estimates. Legacy reference extraction continues only on `notes`/`audio` tracks and existing history provenance paths.

The new `recording` payload and its nested semantic structs reject unknown fields and unsupported versions instead of silently accepting future behavior. Store additional opaque metadata at track or project extension points. Future new semantic fields require an explicit compatible version decision; they must not seize an existing opaque field on a legacy kind.

## Data representation

An example recording track is:

```json
{
  "id": "vocal-recording",
  "name": "Original vocal",
  "kind": "recording",
  "gain": 1.0,
  "muted": false,
  "locked": false,
  "armed": false,
  "recording": {
    "version": 1,
    "clips": [{
      "id": "vocal-clip-1",
      "source": {
        "takeId": "take-1",
        "manifestSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "streamId": "microphone-1"
      },
      "range": {"unit": "audio_frames", "start": 12000, "count": 89280000},
      "position": {"frame": -24000, "sampleRate": 48000},
      "fades": {"inFrames": 4800, "outFrames": 9600, "curve": "equal_power"}
    }]
  }
}
```

The example hash is illustrative. The audio range represents 31 minutes at 48 kHz. A valid-looking hash is not proof the source exists.

| Rust type | Meaning |
| --- | --- |
| `TakeReference { take_id: TakeId, manifest_sha256: String, stream_id: String }` | Exact sealed take/manifest/stream identity. The manifest digest is the recording manifest's verified, domain-separated `sha256` value, not an invented file path or an assumed plain JSON checksum. |
| `TakeRange::AudioFrames { start, count }` | Half-open interval of immutable source audio frames. |
| `TakeRange::Events { start, count }` | Half-open interval of source event ordinals. Neither value is an audio frame count or a musical tick. |
| `TimelinePosition { frame: i64, sample_rate: u32 }` | Explicit signed arrangement placement in frames at the specified rate. Negative placement is permitted. |
| `AudioFades { in_frames, out_frames, curve }` | Non-destructive gain envelopes in source audio frames; `linear` or `equal_power`. |
| `TakeClip { id, source, range, position, fades, edit_source }` | Arrangement view over an immutable source; optional versioned original settings exist only in edited payload 2. Omitted/null fades mean no fade. |
| `RecordingPayload { version, clips }` | Version1 original views or version 2 edited audio views. Both remain readable; empty lists are allowed. |

Source range count must be positive, and checked `start + count` must fit `i64::MAX`, matching the current journal's source counters. This bound does not truncate native host timestamps: their full domain-specific precision remains in the referenced original. Audio placement plus count must fit the signed arrangement timeline. Sample rates are 8,000 through 768,000 Hz. Fade lengths must add without overflow and fit within the selected audio window.

New recording track, clip and stream IDs contain 1–96 ASCII alphanumeric characters or `-_.:`. Take IDs use the existing checked `contracts::TakeId` grammar. IDs are lookup identities, never authorization or paths; storage must not concatenate them into unchecked filesystem locations. Clip IDs are unique across **all recording tracks in a project**, not just within one payload. Manifest digests require exactly 64 lowercase hexadecimal characters.

For audio, initial publication requires arrangement sample rate to equal the source PCM rate. No implicit resampling or time stretching is authorized. Changing trim, position or fade settings changes a view; it does not rewrite original chunks, host timestamps, manifest provenance, or the source range's units.

For raw-event-only recordings, the signed arrangement position is an explicit user/application placement. It is not a measured calibration or a source audio alignment claim. Event ordinals do not supply a duration in frames. Retained host clocks/anchors must be resolved explicitly by a later scheduler. Adding this reference creates no audio stream, interpreted note, guessed pitch, editable expression estimate or synthesized timing evidence.

## Versioned clip edits

The [clip backend](NATIVE-CLIP-EDITING.md) now explicitly adds recording payload 2.
Version1 rejects the new clip.editSource field even when null. Version2 admits a
validated { version:1, original:{range,position,[fades]} } snapshot with exact raw
presence and opaque outer extensions. Update/Restore retain the source reference,
clip ID, audio-frame unit and sample rate; Restore retains the snapshot and v2.
Only the selected raw view is updated, preserving sibling omitted/null fades.
Both current and original windows must resolve against the same sealed stream.

New publication/append remains version 1 and rejects caller-supplied snapshots;
existing version 2 tracks retain their version when another original is appended.
The optional projection is omitted from old canonical requests when absent.
`original_view()` provides a validated semantic source view, not a replacement
serializer for raw project data. The current 85ef… packaged note-editor binary
predates this backend; native clip controls and their new build remain pending.

## Document operations and bounds

`append_take_clip(&mut self, track_id, track_name, clip)` requires schema 2. It creates a new recording track when the ID is absent. For an existing track, it appends only if the kind is recording and it is unlocked; it retains that track's name and opaque metadata. It never converts an existing notes/audio track or silently upgrades schema 1. New track names must be nonblank and at most 160 characters.

The operation builds and validates a cloned candidate, including global clip IDs and serialized metadata size, before replacing the document. Rejection leaves the document unchanged. It does not increment revision, write history, resolve a manifest, save a file or publish a take. It also does not silently change `bars`; publication changes only the explicit schema and recording fields. Arrangement extent can be edited explicitly.

`take_clips() -> Result<Vec<TakeClip>>` returns all typed clips in track/clip order. `referenced_takes() -> Result<BTreeSet<TakeReference>>` returns distinct source identities. Both return empty for schema 1 and inspect only declared schema-2 recording paths. They do not inspect unknown metadata or fetch source objects.

| Limit | Schema 1 | Schema 2 |
| --- | ---: | ---: |
| Project JSON metadata | 1,900,000 bytes | 8 MiB |
| `bars` | 1–128 | 1–`u32::MAX` |
| Tracks | 64 | 64 |
| Native take clips | Not supported | 16,384 total |
| Embedded legacy raw capture | 4,096 events | Same legacy field limit; long native events use sealed references |
| Legacy audio asset | Existing 30 MiB per-asset bridge limit | Same limit for legacy audio fields; native originals use the separate take resolver |

`MAX_PROJECT_BYTES` retains its schema-1 meaning. `MAX_NATIVE_PROJECT_BYTES` is the absolute native metadata read bound; `schema_version()` and `metadata_limit()` expose the selected document policy. `from_json` bounds the input before parsing, applies the schema-1 input bound when applicable, and validates the size of the serialized representation. `from_value` and `to_json` also enforce the selected serialized metadata ceiling. A valid compact document that cannot fit its canonical pretty-printed representation is rejected before publication.

Neither the 8 MiB metadata limit nor the old 128-bar bound limits the duration or total payload size of a native take. Long originals stay in independently bounded, immutable chunks and a verified sealed manifest. Existing schema-1 note, clip, input-capture, import and provenance behavior is unchanged.

## Required resolver and atomic publication boundary

Core validates structure, arithmetic and collision rules. Before committing a project containing a native reference, the store resolver must:

1. Resolve the exact take identity and manifest digest to owned immutable storage; verify the manifest, descriptor and ordered original chunks using the recording implementation.
2. Find the declared stream and verify its unit matches `AudioFrames` or `Events`.
3. Require range start to be at least the stream descriptor's initial source position and range end to be no greater than its verified sealed prefix. Retain explicit gaps/interruption receipts; a range containing a recorded gap does not establish uninterrupted capture.
4. For audio, enforce source PCM sample-rate equality with the arrangement rate. Preserve raw-event clock observations independently of arrangement placement.
5. Atomically publish the source reference, project revision, history and durable operation receipt, with idempotent retry semantics. Failure must leave the prior project visible and preserve the sealed original for recovery.

The coordinator owns that resolver, object storage and transaction implementation. Neither `append_take_clip` nor successful `ProjectDocument` parsing substitutes for it. Editing/removing a clip must not delete the only original; history and retention policy remain separate ownership decisions.

## Verification scope

Core fixtures cover 31-minute audio and more than 4,096 raw events through small references; signed positions; explicit upgrade with opaque name collisions, exact large numbers and original snapshots; schema/payload/unit/hash/range/fade validation; global duplicate identities; atomic rejected append; schema-specific metadata limits; and extraction only from declared paths. These are structural fixture tests, not an elapsed recording, a real microphone/controller delivery or proof of a measured timing path.

Run the coordinated core/workspace test and Clippy gates before recording implementation acceptance. Store publication and recording snapshot tests independently verify the source-binding and atomicity requirements above.
