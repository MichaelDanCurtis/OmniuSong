# Independent performance projection source review

October 22, 2026. Astra architecture review against the working tree based on `70da4b11b54102f480a6dba2e70a781df5431d91`, including the coordinator's final rejected-batch ordering fence. **Decision: accept the bounded shared-reader/projection source boundary; no remaining source blocker identified.** This is not acceptance of the complete editable-performance workflow or a substitute for the final stable-source gates.

The reviewer implemented the mechanical shared-reader extraction earlier in this slice, then independently reviewed the projector, coordinator integration and recording identity changes. The extraction therefore has mechanical compatibility evidence and a subsequent review, but this reviewer is not an independent author of every extracted line. This review changed only this document. No Cargo, formatting, build, native UI, device, input/output, regular-store or live-database operation was performed by the reviewer.

## Reviewed boundary

Reviewed the five frozen projection files: `crates/omniusong-performance/src/projection.rs`, `projection/arithmetic.rs`, `projection/lifecycle.rs`, `projection/tests.rs` and `projection/lifecycle/tests.rs`. Checked their contracts against the shared `live_types`, `performance_tap`, `performance_records` and `performance_inspection` modules, `omniusong-engine/src/live.rs`, engine compatibility reexports and `performance_writer::seal_native`. Also reviewed `omniusong-recording/src/snapshot.rs`, `snapshot_identity.rs`, `snapshot_identity_tests.rs` and the existing governor and note-value bounds.

The dependency direction is sound: `omniusong-performance` depends on core and recording, without an engine/store/device edge. Pure owned DTOs, codecs and contextual verification are shared. Realtime rings, callback ownership, writer lifecycle and the crate-private `NativeSealPermit`/`seal_native` path remain in the engine. Public inspection helpers and successful projection results do not grant normal-seal or publication authority. Existing external engine module paths remain compatibility reexports.

`project_sealed_performance_bounded` accepts a held bounded `SealedJournal` and an explicit versioned request. It returns one complete review or an error. It does not write notes, copy/adopt a source, mutate a project, render audio, open a device or return a partially usable candidate after a refusal.

## Source and semantic checks

The projector uses the reader's existing cumulative sticky governor throughout. It independently authenticates the owned performance profile and complete selected six-stream checkpoint, then matches take, descriptor, manifest, original-row, physical-file, checkpoint and exact recovery-prefix pins. The selected manifest cannot silently exceed or shorten the requested authenticated prefix. A region must be positive, half-open and within that checkpoint. Selected handles are a nonempty sorted unique subset of the four prepared sources. Strict take grammar, lowercase SHA-256 values and version checks precede interpretation.

GUI AppliedEvent witnesses are checked against the control stream by the shared verifier. Derivation uses each authenticated Applied control batch once. Discarded/rejected packet bodies remain retained originals; the projector does not slice or index their event arrays. The final rejected-batch fence refuses an Applied packet after any Rejected packet in that callback before creating an observation, matching the renderer's immediate break on rejection. Panic callbacks also refuse Applied packets.

Voice identity includes the take, worker/session/Panic scope, source, connection generation and original note/voice ordinals. Sequential ownership counters prevent voice reuse inside a scope. Panic clears voices and sustain; counters reset only when the exact next generation is acknowledged. Source observation ticks and GUI ordinals remain continuous. Different sources and Panic generations can reuse numeric ordinals without merging notes. Repeated pitch and zero velocity retain their own identities.

Onsets and gate endings use the applied output-frame axis. GUI ticks remain uncalibrated observations. Sustain belongs to its exact source. The first actual release trigger closes the gate; later release-envelope completion is a separate retained observation. Pulse/drum automatic releases require an observed transition at the actual scheduled frame and velocity, and an earlier explicit release suppresses that transition. The first physical key-up remains separate even when it arrives after automatic release or `VoiceEnded`; repetition cannot replace it.

The lifecycle uses the same `f32` attack/release formulas and iteration order as the current renderer. Advancing to frame T processes frames before T; a retained `VoiceEnded` validates the next decrement at T. This avoids both an extra release frame and treating a release tail as editable-note duration. Steady held/sustained spans do not rerender every frame. Per-advance attack work is bounded at 32,768 steps and release work at 1,000,000 steps, exceeding the current renderer's supported envelope durations while remaining governed. Missing, early, duplicate or late transitions refuse.

Buffered clears are applied by effective frame before same-frame transitions. Panic clears occur at the callback boundary; Finish cuts occur after the callback's audio. End active/tail counts and Finish cut counts must match the reconstructed voices. Gate-ending cuts and later cuts of already-releasing tails retain distinct evidence. This is validation of retained lifecycle semantics, not acoustic equivalence between saved-note synthesis and the original PCM.

## Timing, output and resource limits

Tempo is pinned as exact finite binary64 bits in 30–300 BPM. The conversion uses a checked `u128` rational with 960 PPQ and the source sample rate, never a large-frame `f64` product. Both endpoints round independently to nearest, ties up; duration is their difference. Destination addition and duration subtraction are checked. Zero-tick intervals follow the explicit collapse policy and are never padded. Crossing notes are omitted or refused, never trimmed. Unknown endings require the incomplete policy; a pre-region unknown note also requires the crossing policy.

The following limits are actual admission/refusal bounds, not a capacity or latency guarantee:

| Boundary | Current limit/behavior |
| --- | --- |
| Detailed selected/intersecting observations | 20,000 total, covering emitted notes and selected omissions |
| Active source lifecycle | 64 voice slots, four prepared sources |
| Pre-region uncertain intersections | At most 64 provisional observations occupying their actual voice slots |
| Callback | At most four packets of 32 events; two clears; 128 transitions |
| Retained control group | At most 63 members and 8 MiB of control payload, within the existing authenticated group bound |
| Complete canonical review | At most 8 MiB, including the eventual 64-character digest field |
| Musical values | Start tick at most 1,000,000,000; duration 1 through 100,000,000; valid pitch and full 16-bit velocity |
| Recording governor ceilings | 512 MiB physical file, 262,144 physical rows, 1 GiB cumulative work, 4,194,304 row visits, 524,288 chunk visits, 2,097,152 record visits, 65,536 groups and 32,000,000 SQLite VM steps |

Out-of-document-range musical values refuse the whole request under either omission policy. The work/visit ceilings apply across open, authentication, rereads, decoding, lifecycle validation and output; they can refuse before any individual source or output maximum is reached. They are accounted-work limits, not measured peak RSS or wall-clock guarantees. Callback vectors grow under bounded prepaid reservations rather than allocating the maximum transition array on every empty callback. Allocations, copies, envelope loops, ordering and canonical hashing use the same governor. Source-wide counters use `u64`; narrow result counters remain bounded by the 20,000-row cap.

Excluded sources and wholly outside-region onsets do not consume the selected detail cap. They still undergo source/lifecycle validation, contribute exact source-wide counts and feed a domain-separated streaming exclusion digest. Pre-region observations are promoted only when their intersection is known or their selected ending remains unknown. Thus more than 20,000 source onsets need not exhaust a small chosen view, although all source authentication and work ceilings still apply. The pure regression for this boundary is not a thirty-minute authenticated-source acceptance test.

Selected omissions now retain physical release, gate-ending cause/record, observed voice end and hard-cut evidence when present. Conditional hard-cut disclosure scans both emitted notes and omissions. Unknown-tail disclosure follows the recovered/interrupted manifest status. Full original bytes remain authoritative; the compact exclusion digest does not replace them.

Review identity is a domain-separated digest of the deterministic typed serialization with `review_sha256` empty, then filled with its hexadecimal digest. The final byte bound reserves those 64 characters. Ordered observations, request, policies, exact tempo bits, source pins, counts, omissions and disclosures participate. Original voice identity hashes remain stable when only destination placement changes, while the review digest changes. Future consumers must use this typed convention rather than assume a reordered generic JSON `Value` has identical bytes; they must recompute source semantics rather than trust caller-supplied hashes or notes.

## Physical identity and governor finalization

The bounded reader captures ordinary-file metadata before SQLite opens, then verifies metadata and SQLite's actual `SQLITE_FCNTL_HAS_MOVED` result after establishing its held snapshot. Unix checks include device/inode, link count, length, mtime and ctime. New SQLite sidecars, aliases, path replacement and same-inode modifications refuse. Checks surround successful inspection/chunk returns and complete visitation, including after the last visitor. The shared performance inspector checks again after its semantic pass; the projector checks again after constructing and hashing the final review.

The unsafe file-control call uses the synchronously borrowed live connection handle, the literal main-database name and a correctly typed/aligned `c_int` output pointer. Failure or unsupported VFS support refuses explicitly. No auxiliary held file descriptor is treated as a SQLite exclusion lock. The existing physical SHA reader can open/close its own file descriptor, so this proof depends on the retained metadata/SQLite-handle checks and final recheck, not an assumption of uninterrupted POSIX read locks. Legacy unbounded reader/export behavior retains its prior scope. Actual Windows identity support remains unqualified.

Sticky cancellation/resource refusal wins at finalization and returns no usable review. The result does not grant later mutation authority: publication must reopen/revalidate the exact registered source under its own current operation boundary.

## Issues found and resolved during review

1. A source-wide 20,000-observation cap could reject a tiny region from a larger source. Detailed storage is now selected-only, with bounded provisional slots, source-wide counters and streaming exclusion identity.
2. Pathname hashing could mix a replaced physical file with an older held SQLite snapshot. The bounded physical identity checks above and post-digest check close that path.
3. A selected collapsed/crossing omission lost its actual engine-cut cause, and hard-cut disclosure scanned only emitted notes. The immutable omission fields and conditional disclosure now preserve that evidence.
4. Applied records after a same-callback Rejected record could describe execution the renderer cannot perform. The final callback rejection fence refuses before event interpretation. Its regression intentionally uses a malformed rejected packet to preserve the opaque-payload rule.
5. Contract wording could imply omission permits out-of-range `NoteValues`; it now documents whole-request refusal. The integration plan now explicitly makes durable exact-request admission a prerequisite to native restart retry/reconciliation.

No remaining blocker was identified in the bounded source reviewed here. This conclusion is limited to the current owned profile and renderer semantics and does not assert that every conceivable forged recording has a dedicated adversarial test.

## Evidence and required follow-on gates

Read the completed coordinator-run `.runtime/rust-performance-projection/initial-projection-tests.log`: 29 low-crate unit tests and one fixed-byte compatibility test passed; doc-tests contained zero tests. That initial run predates the final rejected-batch regression/fence and cannot qualify the final source. The fixed oracle binds the retained descriptor and eight v2 records to fixture SHA-256 `6f1bc73da795dfc09ab5f317ac6ce02f0df84b766ed8dc14aa35d3b9fc5ee3e9`; its source raw-file identity is `f6e5aef06a0abf7fa020731eb1f1023e2ca7a5847f581651b129cd92c248a111`. The earlier extraction audit records mechanical codec/native-seal-prefix preservation before formatting. Those are specific compatibility checks, not proof of all future wire changes.

Reviewed the six actual synthetic Unix identity test implementations: unchanged reads/visits, same-size different-SQLite replacement, same-inode write with restored mtime, new sidecar, replacement in the last visitor and sticky cancellation preceding a later path error. No test was executed by this reviewer. Final coordinator-run workspace/optimized gates and the separate renderer→writer→recovered-source integration/adversarial suite must establish executed results against stable final source. Mutable or unrun integration tests are not predicates of this source acceptance.

The [integration plan](RUST-PERFORMANCE-PROJECTION-PLAN.md) and [contract](../contracts/NATIVE-PERFORMANCE-PROJECTION.md) remain binding next work: explicit schema-3 projection registry and typed source ownership; one recomputed atomic publication operation; replay, Undo/Redo and durable exact-request uncertainty; full OSAR/OSRE/copy closure; guarded native review/confirmation and ordinary-note editing; deliberate original-versus-derived audition. A sequence of AddNote calls, a caller hash or an opaque note extension cannot substitute for these requirements. Durable admission must precede enabling restart retry/reconciliation, and historical replay must not reinstall over later edits or Undo.

This report establishes no native control, installed binary, microphone/controller/loopback input, playback/listening, thirty-minute capacity, calibrated timing, MIDI 2/MPE expression, Windows acceptance, archive publication or completion of milestone 2. Those remain separate required gates in the full Mac-first Rust workstation goal.
