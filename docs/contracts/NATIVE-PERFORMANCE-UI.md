# Native recorded-output controls and retained intent

October 20, 2026. This contract extends the [native performance session](NATIVE-PERFORMANCE-SESSION.md). Implementation and qualification are separate gates. It does not complete milestone 2.

## Scope

The musician explicitly records the procedural keyboard/pad output bus. No native input stream, including virtual input, opens in this qualification. Audition and recording remain visibly different modes. A recording retains the complete six-stream original; selecting audio for a song does not discard the original GUI/control evidence. Editable notes require a later explicit derivation.

The sequence is Record keys/pads → persist Start intent → real asynchronous Ready → performance → Finish → persist exact Finish intent → native Finish/seal → reliable Stop acknowledgement → retained original → explicit attachment choice → persist complete publication intent → execute the stored publication. Stop is available throughout and bypasses storage. Neither Finish nor reopening the app automatically attaches, resumes, rerenders, recovers or seals a source.

## Storage ownership and local ledger

The existing serialized `SessionService` owns all intent persistence. UI paint/input does no file access, hashing, SQL, or source inspection. `omniusong-session` may reference the Rust engine's strict control DTOs; the dependency direction is session → engine → store, with no cycle.

Native catalog schema 5 adds `recording_intents(id, revision, payload, payload_sha256)`, keyed by `(id, revision)`. UPDATE and DELETE are forbidden. Existing project, edit, operation, original-take and archive-lineage content is retained. This local studio ledger is outside portable per-song archives: archived songs retain attached originals/history; unassigned sources and local pending choices need separate export/retention work.

Bounds are 32 distinct Start identities, 16 append-only revisions per identity, and 16 KiB per payload. Reaching a bound rejects the new choice; it never evicts evidence. Store reads authenticate row identity, sequence and payload hashes. Session reads also deserialize every versioned historical payload and validate every transition. Corruption fails closed and remains visible; this bounded packet does not provide damaged-card salvage.

`StoredPerformanceIntent` is the storage envelope. Store append uses an immediate transaction and verifies the committed candidate before commit. An exact latest payload retry returns the same revision even with an older expected revision; a different stale payload is rejected. A digest alone never supplies intent equality.

The strict session schema is:

- `PerformanceIntent { version: 1, start: PerformanceStart, project_id: ProjectId, journal_path: PathBuf, descriptor_sha256: Option<String>, finish: Option<PerformanceFinishIntent>, attachment: Option<TakePublication>, released: bool }`.
- `PerformanceFinishIntent { id: LiveId, scope: LiveScope }`.
- `PerformanceIntentReceipt { revision: u64, intent: PerformanceIntent, payload_sha256: String }`.

Start/output/clock/project/path are immutable. Path must equal the opened store's canonical root plus `captures/<32-lowercase-hex-start>/take.sqlite3`; it need not exist before Start. Descriptor, Finish and publication fields may each change from absent to one exact value, then remain immutable. Release is irreversible and does not remove the source or any ledger revision. It does not imply the stage existed, capture succeeded, the worker stopped, or a native seal capability existed.

Release after an attachment choice requires a verified exact durable project-operation receipt. An absent or uncertain operation cannot be released merely because the UI received an error. A known revision conflict retains the original choice; replacing it with a new publication is a separate future workflow. This packet must not claim a complete conflict/superseding-choice experience.

## Commands and side effects

`SavePerformanceIntent { expected_revision: Option<u64>, intent: Box<PerformanceIntent> }` returns `Reply::PerformanceIntent`. Shape/semantic failures before any write are known rejections; SQL, transport and lost-response outcomes stay conservative. An uncertain save keeps the exact payload and CAS expectation for explicit retry.

`ListPerformanceIntents` returns at most 32 current typed receipts after authenticating the bounded history. Listing never invokes the audio engine. A reopened card is retained-intent evidence, not evidence of a normal seal or an active recording.

`AttachPerformance { id: LiveId, revision: u64 }` loads the exact current stored publication and path. It never accepts substituted caller metadata, silently changes an expected project revision, or guesses a recovered range. Existing verified attachment/publication operations supply atomic project/history/operation receipts and owned-original retry behavior. A replay loads the current saved document, not an old committed snapshot. Ingestion can have committed before a later publication conflict.

Initial performance attachment is normal-source-only in this packet. It requires the saved descriptor digest, strict v2 performance profile, exact original GUI clock/take, authenticated manifest, complete format-aware six-stream checkpoint coverage, matching stored Finish/callback cut, and no capture fault. The read-only `SealedJournal::inspect_snapshot` and `inspect_sealed_performance` APIs share the original validators without acquiring an owner or creating sidecars. Already registered originals take precedence over staging; a damaged owned original cannot fall back to a convenient stage. A committed operation is reconciled before source access for exact replay/release, without claiming present source availability. These checks validate existing originals; they supply no native seal authority. Recovered performance attachment needs its later reviewed bounds and remains unavailable here.

Every engine or attachment continuation is bound to the authenticated session-service completion and the exact pending command/card. Stop cancels a queued Start/Finish continuation immediately, including one whose save later commits. Reopening only exposes retained choices; it never plays an old intent automatically. A persisted Finish scope is not rebased across Panic or a new worker.

Entering retained review requires a clean, available session and no active audio/modal. An unresolved stored attachment requires its original target project. Review selection never publishes: an explicit stored-attachment retry sends the existing ID/revision, and main rechecks that target and all draft guards. Fresh never-started choices rejected before any write may be dismissed locally; prior uncertain outcomes, authenticated receipts or any engine Start attempt prohibit this escape. Dismissal clears only the unstarted GUI state and never deletes stored evidence.

## GUI, Finish and Stop

The engine port exposes coalesced performance status separately from ordinary replies and cumulative live/urgent status. Only the first authenticated real Ready can activate the prepared live controller. Repeated status does not reset clocks, sequence, voice handles, credits or Panic generation. Newer faults cannot be hidden by an older Start completion.

Finish first closes local note and lease generation, flushes the finite staged events once with their original fields, and retains the exact Finish choice. It must not use generic cleanup that clears originals, inserts NoteOff or requests Panic. Held/sustained voices end at the callback-owned cut. A failed flush interrupts and discloses loss rather than retrying an unknown subset. Storage stalls may expire the lease and interrupt recording; no successful tail is fabricated.

Reliable Stop requires the exact cumulative urgent acknowledgement or typed actor retirement. Zero voices, a `Stopping` status, successful pause flags or a sealed manifest do not supply that acknowledgement. New Start and source-owner reuse stay disabled until quiescence is proven. A sealed source can still be temporarily owned by the final writer; release that worker via reliable Stop before exclusive inspection.

GUI-local discarded events, parent-queued discarded events, worker acceptance and callback-retained originals are distinct. Count actual locally discarded staged events with checked arithmetic and preserve the first reason. Until parent discard telemetry is implemented, show that count as unavailable/unknown. Never derive event counts from sequence subtraction or label every key/pad press saved. Normal sealing describes the retained callback original, not completeness of GUI activity before admission.

## Review and acceptance boundaries

Keep Record, Finish and Stop obvious and accessible near the instrument. Present preparing, recording, finishing, saving, retained and attachment states in plain language. Separate callback-rendered and writer-durable frames. Raw paths/hashes belong in expandable evidence. Keep originals/errors visible after worker failure; a missing descriptor means uncertainty, not absence.

New UI acceptance requires headless delayed-Ready/Stop/fault/intent tests and an actual same-binary Mac output-only walkthrough, then explicit attachment, Undo/Redo, restart and full attached-original archive restoration. Preserve the preexisting catalog and prototype files. Verify persisted pending intents independently after restart. Retain exact source/build hashes and failed attempts.

Performance-aware source inspection/checkpoint recovery, finite parent-loss telemetry, corrected attachment choices after conflict, and editable-note projection remain separately qualified stages. Generic UUID-only capture recovery must not be weakened or used to seal a performance stage. No microphone, physical MIDI, listening, latency, sustained recording, inference, plugin, installer or Windows acceptance is implied by this packet.
