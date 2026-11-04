# Initial Rust architecture and persistence review

Reviewed October 20, 2026 against the working tree. This is an independent source and contract review, not runtime qualification. The coordinator and UI agent were still editing their owned files during review; function names below identify the relevant code if line numbers move.

Scope: the full [goal](../GOAL.md), [release matrix](../RELEASE-ACCEPTANCE.md), [architecture](../RUST-ARCHITECTURE.md), [migration plan](../RUST-MIGRATION.md), root dependency declarations, and the complete initial `omniusong-core` and `omniusong-store` implementations and tests. The review also compared the legacy document validator and clip contracts and consulted the three native research reports. App source was read only where a store contract affects startup/recovery. No code, original data, devices, dependencies, or hardware permissions were changed. No build or tests were run for this review; reported coordinator test results are not independent test execution.

## Decision

The proposed direction can support the full goal. Rust native UI, portable document ownership, SQLite transactions, bounded realtime data, separate plugin/model supervision, and independent MIDI qualification stages are appropriate foundations. None of the selected UI, storage, or native-library technologies inherently requires reducing the product.

The initial architecture milestone should remain **in progress** until the remaining validation and shared-contract work below is addressed and the coordinator records fresh integrated verification. Source-preservation fixes were incorporated during review; their current disposition is recorded below. This is not a finding that all audio, MIDI, plugin, or model features should already exist. The initial shell can continue to be integrated while those implementation milestones remain pending.

## Findings and current disposition

| Finding | Latest source-review disposition |
| --- | --- |
| R1 Opaque metadata interpreted as media | Addressed in source: explicit schema-owned references replace recursive guessing. Fresh integrated verification pending. |
| R2 Existing import metadata overwritten | Addressed in source: the prior value is retained under `nativeImport.previousImport`, with exact source bytes separately retained. Fresh integrated verification pending. |
| R3 Known musical invariants omitted | Partially addressed: clip windows, fades, original snapshots, and transport checks added. Recording-timing parity still open at last inspection. |
| R4 One damaged session aborts listing | Addressed in source: per-session problem summaries preserve healthy listing. Fresh integrated verification pending. |
| R5 Shared contracts not adopted | Open. Signed timeline/source-frame distinction addressed; process/protocol/ownership decisions remain to be adopted. |
| R6 Recording-scale storage contract | Open dependency before recording implementation; schema-1 bridge limits remain appropriate for legacy imports. |
| R7 Evidence/platform/build traceability | Open at last document inspection. |

The detailed findings below describe the initial reviewed source and the reason for the requested change; the table and closing source notes distinguish those findings from the current source state.

### R1 — High: opaque metadata is interpreted as a local media declaration

**Location:** `crates/omniusong-core/src/lib.rs`, `referenced_assets` (reviewed lines 244–284); `omniusong-store::import_legacy`.

The recursive walker treats any property ending in `assetId`, anywhere in the JSON tree, as a local content-addressed media identifier. It also treats any string matching a hash-and-audio-extension pattern as a reference, irrespective of its field meaning. Consequently:

```json
{
  "vendor:provider": {"assetId": "remote-provider-recording-123"},
  "vendor:example": {"documentationValue": "<64 lowercase hex characters>.wav"}
}
```

The first value fails project validation; the second requests a local file even though the extension has not declared a local asset. Unknown metadata preservation is therefore not just a serialization concern: valid opaque data can prevent import. Hash validation must continue for actual local references, but arbitrary extension field names must not assign them that meaning.

**Action:** enumerate schema-1 local reference paths, including known retained candidate/provenance input paths. Define a versioned explicit asset-reference declaration for future extensions. Preserve undeclared namespaces as opaque JSON. An extension-specific reference resolver can add declared dependencies without recursively guessing their semantics. Do not drop legitimate historical source assets while narrowing discovery.

**Acceptance:** migrate fixtures containing opaque remote `assetId` fields and hash-like descriptive strings; retain them exactly without requiring a file. Separately prove that track assets and known historical/provenance source assets are copied, and malformed/path-traversal references in those declared locations are rejected atomically.

**Gate:** A05, R04, R06; resolve before describing migration as lossless.

### R2 — Medium: an import overwrites an existing unfamiliar field

**Location:** `crates/omniusong-core/src/lib.rs`, `imported_copy` (reviewed lines 235–241).

`value["nativeImport"] = ...` replaces an existing field without retaining its prior value in the editable imported document. Schema 1 does not reserve that property, and the stated contract says unfamiliar fields survive. The exact original JSON receipt remains recoverable in the database, which prevents loss of the only source bytes, but that does not make the imported musical document lossless. Reimporting a previously exported native document can also discard earlier import lineage.

**Action:** keep the current import receipt outside source-owned JSON, using the existing `imports` table and a typed receipt API, or introduce an explicitly migrated and collision-safe application namespace with retained prior lineage. Do not silently overwrite an opaque field.

**Acceptance:** import a valid source with a preexisting arbitrary `nativeImport` object; compare that original property after import, rename/save, reopen, and undo. The new import receipt must still be accessible independently.

**Gate:** A05, R04, R06.

### R3 — Medium: the “validated schema-1 document” permits malformed known clip and transport state

**Location:** `crates/omniusong-core/src/lib.rs`, `validate`, especially the audio branch (reviewed lines 353–372); compare `src/project.mjs:71` and `:86–98`, `public/clip-edit.js`, and the legacy transport/recording-placement contracts.

The native validator checks source duration, byte length, and optional start tick but does not check existing trim/fade fields, original clip snapshots, recording timing, or transport settings. For example, an audio object with `durationSeconds: 1` and `trimEndSeconds: 100` passes this branch, although the legacy validator rejects it. An invalid original clip snapshot also passes. This is a gap in a current known schema, not a request to validate every opaque vendor field.

Such a document can be imported and durably saved under a type that downstream code is invited to treat as validated. When the native editor or engine starts consuming these fields, unchecked bounds can produce invalid playback spans or unrecoverable “restore original” actions.

**Action:** port the known legacy invariants and their focused fixtures. Alternatively, explicitly separate an opaque archival document from a validated editable/renderable projection, with typed errors and a visible repair workflow before it can be scheduled. Do not imply validation of musical fields that the type merely retains.

**Acceptance:** valid trimmed/faded/compensated imports remain unchanged; invalid clip bounds, excessive combined fades, malformed original snapshots, impossible recording-placement state, and invalid transport settings are rejected or quarantined without mutating the source.

**Gate:** A05, H03, H04, R04, R06; fix before those fields become editable or renderable.

### R4 — Medium: one invalid stored session aborts discovery of every healthy session

**Location:** `crates/omniusong-store/src/lib.rs`, `NativeStore::list` (reviewed lines 168–188).

The list query parses every payload and collects one `Result<Vec<ProjectSummary>>`. A single syntactically invalid or unsupported project row fails the entire list. This prevents callers from showing healthy sessions alongside a damaged session and makes recovery dependent on bypassing the ordinary listing API.

**Action:** select the actual table identifier as well as its payload and return valid summaries plus per-project diagnostics, or a list of typed healthy/damaged entries. A damaged entry should remain identifiable and recoverable without being loaded into the editable document. Preserve a separate whole-database error when SQLite itself cannot safely read.

**Acceptance:** corrupt one project payload in a disposable test database; other projects remain discoverable and reopenable, the damaged project is identified accurately, and neither row is silently deleted or rewritten.

**Gate:** A06, R05, R08. This is an actionable initial-store recovery improvement, separate from later recording recovery.

### R5 — Architecture acceptance gap: shared contracts remain recommendations rather than adopted interfaces

**Location:** `docs/RUST-ARCHITECTURE.md`, planned boundaries and decisions 3–4; `docs/RUST-MIGRATION.md`, milestone 0/1 exits; `docs/GOAL.md`, shared-contract requirement.

The research supplies useful illustrative contracts, but the decision document does not clearly adopt or reject several choices that independent implementers need:

- Whether the native engine is a supervised process or a thread in the application; ownership of device lifetime and recorder recovery after UI failure.
- Exact control/acknowledgement semantics: queued, adopted at render revision, audible, and durably committed are different states.
- The initial plugin isolation topology and its scheduling/latency policy. A separately scanned plugin is not necessarily isolated during processing. Worker IPC must have bounded audio/event capacity, an explicit miss policy, and declared latency.
- Versioned edit, job, candidate, extension, and external worker envelopes. Whole-document `save` with an expected revision is useful storage machinery, but it is not the complete scope/lock/idempotent-acceptance contract.
- Source identity, playback voice identity, expressive units/scopes, and clock epoch representation at the engine/MIDI/plugin boundaries.

The initial core also had `SampleFrame(pub u64)` versus the research's signed timeline frame for preroll. This has been addressed during review with signed `SampleFrame` and separate unsigned `SourceFrame`. A bare frame number still needs a sample rate/clock epoch in its enclosing context.

**Action:** coordinator records an adopted minimal contract revision, including concrete signatures or serializable examples and ownership/failure semantics. The [audio research](../research/RUST-AUDIO-MIDI-PLUGINS.md) already provides most of the material. Keep internal modules together if appropriate; adding empty crates is unnecessary. Require the contracts before parallel recording/plugin/job implementations.

**Acceptance:** another agent can implement a device adapter, recorder, or worker without inventing clock units, identity rules, message versioning, state transitions, or crash behavior. Include one example covering a repeated note with sustain, a device restart, a late plugin block, and a stale candidate.

**Gate:** A04 and the explicit milestone 0/1 contract exits. This is not a requirement to implement the later systems in milestone 0.

### R6 — Required next-milestone decision: schema-1 storage limits cannot become the recording contract

**Location:** core `MAX_PROJECT_BYTES`, `validate` raw-capture bound; store `MAX_ASSET_BYTES`, `MAX_TOTAL_ASSET_BYTES`, `check_references`, and `import_legacy`.

The initial bridge limits an asset to 30 MiB, an import's combined media to 48 MiB, project JSON to 1.9 MB, and embedded capture to 4,096 events. The store materializes asset BLOBs and checks their full content hashes when loading or saving references. These are bounded, defensible legacy-import choices. They cannot be reused as the production long-recording/streaming contract: 30 minutes of stereo 48 kHz 24-bit PCM alone is 518,400,000 bytes before headers, and expressive MIDI can exceed 4,096 events rapidly.

The architecture mentions incremental journaling but does not yet decide its format, ownership, committed boundary, or asset-reference migration. Removing the byte limits alone would leave whole-asset memory, hashing, and transaction work on user-facing operations.

**Action before milestone 2:** keep the legacy bridge explicit and adopt a new native recording manifest with bounded immutable chunks, raw-event chunks, clock receipts, source hashes, durable append/finalize acknowledgement, and recovery state. Decide whether chunk content remains in SQLite or uses staged files plus a safe publication protocol; either can work if demonstrated. Define how snapshots reference assets without rewriting originals and how archives include chunks referenced by undo/history and pending recovery. Schedule integrity verification off realtime/UI paint paths and avoid rescanning an entire long take for each small edit.

**Acceptance:** simulated long capture beyond legacy bounds, bounded peak memory, interruption between chunk write and manifest publication, exact recovered prefix/gap report, save/undo/archive/reopen with retained source chunks, followed later by the actual 30-minute device test.

**Gate:** R01–R07, P02, P03. This is an explicit implementation dependency, not evidence that SQLite is unsuitable or that the initial migration store must already record audio.

### R7 — Architecture acceptance gap: requirement coverage is broad, but evidence traceability is incomplete

**Location:** `docs/RELEASE-ACCEPTANCE.md`; milestone 0 exit in the migration plan.

The matrix correctly preserves the required feature scope and keeps statuses pending. Its columns currently record requirement, evidence required, and state. The goal expressly asks for implementation, test, platform, evidence, and status mapping. A broad row such as E03 or D10 cannot show Mac progress separately from Windows, and “evidence required” is not a link to evidence actually produced. There are also no native build/run commands in the reviewed four milestone documents or README snapshot.

**Action:** add per-platform implementation/test/evidence links or link each matrix row to a structured receipt with those fields. Record native workspace build/test/run commands and native data-directory behavior in a discoverable operator document. Keep partial evidence in progress until the entire row's applicable gates pass.

**Acceptance:** each claimed milestone result points to source, reproducible commands or named test artifacts, target OS/runtime, observed result, and remaining checks. A Mac test cannot accidentally mark a Windows requirement passed.

**Gate:** A04, P04, P08 and milestone 0 exit; documentary completeness rather than missing product functionality.

## Boundaries that the review supports

The reviewed architecture correctly keeps MIDI 2 project precision, UMP codecs, native endpoint transport, MIDI-CI, standard MIDI 2 Clip Files, MPE, and physical/audible qualification separate. It does not mistake the existing UMP JSON trace for a standard file or invent a universal MIDI 2 wire note identifier.

The native-device and plugin research distinguishes actual capability from crate names: a MIDI 1 portability wrapper is not a UMP backend; a codec is not MIDI-CI; a plugin authoring library is not automatically a host; plugin formats have different note/expression mappings. The Windows MIDI deployment caveat is correctly retained as a qualification dependency. A Windows MIDI 1 fallback cannot close mandatory Windows MIDI 2 acceptance.

The realtime prohibition on project/database locks, allocation, filesystem/network access, and inference is appropriate. Model workers must stay outside the device callback budget, and plugin isolation overhead must be counted in software latency. Under-10-ms response and uninterrupted 30-minute capture remain actual measured targets.

Expected-revision saves, history/cursor transactions, exact source receipts, atomic import publication, and a separate native store are sound starting points. Retaining original JSON bytes provides valuable recovery evidence in addition to editable-document round trips. The tests inspect useful behavior, including stale writes and undo branches; they do not establish process-kill, power-loss, hardware, or actual concurrency qualification.

## Changes observed during review

The root dependency now enables `serde_json`'s `arbitrary_precision`, and core includes a regression fixture for opaque integers beyond `u64` and long decimal values. This addresses the initial concern that unfamiliar JSON numbers could be silently rounded. Source inspection confirms the change; its test was not rerun by this reviewer.

The history navigation path now checks that a loaded undo/redo snapshot belongs to the requested project before saving it. This addresses a potential cross-project snapshot integrity failure. Source inspection confirms the guard; corruption-injection verification remains to be performed.

The coordinator then replaced recursive asset guessing with schema-owned track and candidate-provenance input paths. These paths match the prototype's candidate merge behavior. A regression fixture now retains opaque remote asset identifiers and a hash-like descriptive string without treating them as local media. Existing import metadata is retained in the new receipt's `previousImport` field, and the original source JSON remains separately available. This addresses R1 and R2's information-loss issues; document the receipt overlay/lineage semantics as part of the migration contract.

Core now validates trim/fade windows, original clip snapshots, and transport settings; recording-timing parity remains outstanding at the last inspection. `SampleFrame` is signed, and `SourceFrame` is unsigned. The store now selects the actual database identity/revision and returns a `problem` on an individual damaged summary while preserving healthy summaries. These changes address the identified portions of R3, R4, and R5. Their added test source was inspected, but no test outcome is claimed by this review.

## Expected omissions, not initial architecture defects

Native playback/recording, devices, MPE state machines, MIDI-CI, standard Clip Files, plugin hosts, model workers and adapters, full editing, archives, installed builds, accessibility inspection, and Windows behavior are intentionally later work. Their absence does not invalidate a clearly labeled native-shell slice. Actual app launch and UI evidence still belong to the coordinator's integration qualification.

The model research preserves all eight requested roles and identifies real native-runtime/porting gaps without introducing an unapproved Python service. Those gaps remain required work. No model role, plugin format, Windows parity requirement, real controller test, or final creative session may be removed merely to close this initial review.

## Follow-up disposition

For architecture acceptance, verify the R1/R2/R4 fixes, finish R3's recording-timing parity, settle R5, complete R7, and adopt or assign the R6 recording contract before its implementation milestone. Keep A04 in progress until the coordinator records these dispositions and links fresh integrated verification. This review alone does not pass P09's future realtime audio, plugin-isolation, or expressive-MIDI implementation reviews.

## Coordinator disposition after integration — October 20

The [fresh workspace receipt](../verification/rust-native-tests.json) records 39 passing tests, formatting and clippy with warnings denied. R1–R4's initial software fixes now have integrated evidence: declared-only media discovery, prior import lineage/precision after edits and reopening, clip/transport/recording-timing rejection without partial rows, both foreign-history directions, late import-transaction rollback and damaged-row isolation. All 19 original sessions import and reopen independently; all 30 original project/audio hashes still match the initial inventory.

R5 now has adopted [process/protocol decisions](../contracts/NATIVE-PROTOCOLS.md), concrete versioned contracts and eight contract tests; an [independent bounded review](RUST-CONTRACTS-INITIAL.md) found no major protocol/ABI defect. R6 now has a writer-owned per-take SQLite journal design with immutable bounded audio/raw-event chunks and explicit committed-prefix/seal/publication semantics. These address the initial design decisions; IPC authentication/framing, realtime queue enforcement, recording codecs/writer, fault injection and actual device tests remain implementation gates.

R7 now has implementation/test and separate platform columns, reproducible build/run commands, a dependency inventory, hashed workspace and UI receipts and explicit remaining checks. The actual Mac loading window renders and can close while its initialization worker waits on drive access. Actual studio create/import/save/reopen UI remains pending that permission decision. This closes the initial traceability gap without passing the native-shell, installed Mac, Windows or future runtime review milestones. The initial observations above are retained as the review record.
