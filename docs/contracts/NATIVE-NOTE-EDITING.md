# Native note editing

October 20, 2026. Frozen interfaces; library/storage/archive implementation and isolated synthetic qualification complete. This packet extends milestone 2; its integrated build also corrects a focus defect discovered in the prior live-audition app. It does not implement the piano-roll interface, performance recording, quantization, expression-curve editing or expressive MIDI playback. These remain required by [the full goal](../GOAL.md).

## Document identity and finite operations

`DocumentNoteAddress { track_id: String, note_id: String }` addresses a note within one project. Both components match exact schema-owned JSON keys, never array positions or pitches. The same pitch may overlap and the same note ID may occur on separate tracks. Legacy keys permit empty strings, Unicode and control characters, bounded to 200 track-ID and 99 note-ID Unicode scalar values, with corresponding 800/396-byte ceilings. They are never filesystem paths or global transport identities. Existing strict `TrackId` and `NoteId` types remain unchanged.

`ProjectEditIntent` retains its stable operation ID, project ID, expected revision and opaque extensions. Note operations require exactly `EditScope::DocumentNote { address }`. The operation payload cannot carry a second conflicting address or a generic JSON patch:

| Operation | Payload |
| --- | --- |
| `AddNote` | `values: NoteValues`, `timeline: NoteTimelinePolicy` |
| `UpdateNote` | `values: NoteValues`, `timeline: NoteTimelinePolicy` |
| `DeleteNote` | No additional payload |
| `RestoreNote` | `timeline: NoteTimelinePolicy` |

`NoteValues` contains integer `pitch: u8`, `start_tick: u64`, `duration_ticks: u64`, and `velocity16: u16`. Validate pitch 0–127, start 0–1,000,000,000, duration 1–100,000,000 and checked end arithmetic. Preserve velocity zero as a valid high-resolution note value. Do not convert through percentages, floats, grid cells or MIDI 1 semantics. Add additionally requires a fresh strict `NoteId` from the caller and rejects an existing ID within that track. Update/delete/restore accept all valid legacy address keys without remapping.

Operations apply only to existing unlocked `kind: notes` tracks. Do not interpret similarly named opaque fields on recording/audio tracks as editable note data. Refuse missing targets, duplicate add IDs, unsupported scope, invalid values, expression loss, note-count/metadata-limit exhaustion or unchanged complete results. Failure leaves the complete source document and store unchanged.

## Pure changes and original preservation

`ProjectDocument` exposes immutable `add_note`, `update_note`, `delete_note`, and `restore_note` methods. They clone, validate the target and planned result, and return a validated new document. Revision advancement belongs to the store transaction. Full-document parsing, serialization and file verification remain on the storage/preparation worker, never UI paint or sample callbacks.

Update changes only the four musical fields. The first changed musical edit creates version-1 `editSource` with the original four values and an exact expression field only if the original had one. Absent expression, explicit null and an empty array remain distinct. Later edits preserve the whole snapshot, including unknown fields and unfamiliar numeric values. Source identity, physical key duration, release velocity, receipt clocks, raw MIDI/UMP, model estimates and unrelated metadata remain unchanged.

Expression ticks remain relative to the note. Moving/pitching a note does not rewrite its curves or estimates. Duration cannot shorten past the last active expression point; equality at the final point is valid. No points may be dropped, scaled or clamped. Input key-duration metadata is original evidence and must not be rewritten to match a derived duration.

Restore copies the snapshot's known four values and exact expression presence back to the active note. It **retains the entire immutable snapshot**, including opaque fields. A future interface determines whether values differ from the snapshot, rather than treating snapshot presence alone as a modified flag. This avoids the prototype's lossy removal of unknown snapshot metadata. A note without a snapshot cannot be restored. Restore does not rewind raw input or source/model identity.

Delete removes only the selected active note. The existing durable before/after edit history and operation snapshots retain its original. Archive closure must preserve notes reachable only through history or receipts, including after Undo and a new edit branch. Add creates exactly six fields: ID, the four musical values and an empty expression array. It invents no physical performance, input metadata or AI inference provenance.

No-op means the complete planned musical document is unchanged. An explicit loop extension with identical note values is a real timeline change and must not create a first-edit snapshot. Restore with already-original values may still perform a requested loop extension; without any actual change it rejects. Do not add duplicate project `history` entries merely to mirror the transactional journal; unrelated inline history remains exact.

## Explicit loop and format policy

Every add/update/restore request includes one policy:

- `PreserveLoop` leaves schema and bar count unchanged. This permits editing any valid imported note, including a long note outside the current playback loop. A future interface must disclose an out-of-loop note rather than hiding it.
- `ExtendLoop { allow_native_schema_upgrade }` grows to include the note end using checked ceiling division at 3,840 ticks per 4/4 bar. Never shrink existing bars. Within schema-1's 128-bar limit, keep schema 1. Above that limit, refuse unless the explicit flag permits schema-2 promotion; promotion preserves every unrelated field and remains recorded in the operation intent. Existing schema-2 sessions extend within their own validated limit regardless of that flag.

Do not promote merely to bypass a metadata size limit. Deletion never shrinks the loop. Restoring or changing velocity does not silently change the loop or schema. No audio resampling, MIDI clock calibration or project timebase conversion is implied.

## Store, replay and complete archives

Reuse the existing replay-first, expected-revision transaction. Canonical request content and operation identity are checked before current revision. A same-ID lost-ack retry returns its exact original receipt while the session service separately returns the actual latest document; it never reapplies the edit or overwrites later work. Changed request content, cross-project operation reuse and archive-copy ID collisions reject. Project/current history/cursor/operation result publish together; a final SQL failure rolls all of them back.

Durable Undo/Redo and reopen must preserve the same note/source values. New edits after Undo may truncate the active redo branch, but retained operation results and original evidence still belong to complete archive closure. Pure pre-write semantic rejection uses the narrow `StoreError::EditRejected` / `FailureKind::EditRejected` classification with a known uncommitted outcome. Database, verification and postcommit projection failures remain conservative; do not exempt generic `Rejected` errors.

Archive validation checks bounded postconditions for all four operations: exact existing unlocked notes track; Add has its authored six-field shape; Add/Update values match the request; Delete removes the target; Restore matches retained known source values and exact expression presence; explicit extension covers the note end. These checks do not infer an unavailable preimage or previous bar/schema state. Strict v1 reads, OSARv2 exports, database schema 4 and musical schema versions remain separate boundaries; do not relax malformed/unsupported operation validation. Older readers may reject newly recognized edit operations rather than silently dropping them.

Restore-as-copy remaps only owned project/operation identities. Document track/note keys and opaque UUID-looking metadata remain exact. The mapped operation request must still validate and replay correctly. Complete archives include current, history, receipt/import closure and required original objects; note edits must not break existing capture/archive workflows.

## Ownership and qualification

The coordinator owns these types, scope/operation contract, dependency/schema decisions, integration and verification. The implementation agent owns pure core mutations and store dispatch/archive support. A separate integration agent owns background-service tests and an isolated synthetic archive example. The independent reviewer owns adversarial core/store tests and review disposition. Only the coordinator runs Cargo and device/UI checks; no live catalog edits or native input are permitted in these fixtures.

Required software evidence includes exact field bounds/velocities; repeated and cross-track IDs; empty/Unicode/maximum legacy keys; lock/kind/scope refusal; exact absent/null/array expression and immutable snapshot metadata; expression-tail guards; raw packet/clock/unknown-number preservation; explicit loop/promotion/no-op behavior; transactional rollback/conflict/retry; history/reopen; complete identity and copy archive restoration; and unchanged original projects/audio.

The prior `35822214…` bundle completed its output/control/restart walkthrough and exposed a piano-button focus defect. Its source correction passes separate regression tests and the corrected bundle’s own bounded native walkthrough. The [synthetic note-edit receipt](../verification/rust-native-note-editing.json) qualifies library/storage/archive behavior only; it exposes no note editor in the app. Actual native note selection, piano roll, field drafts, Save/Discard/Cancel, focus/navigation/reconnect, rendered agreement and restart remain subsequent integrated interface work. The new note draft must participate in the existing audio/recovery/session guards; metadata saves cannot silently discard it. MIDI 2/MPE expression delivery, listening quality and Windows remain independent unfinished gates.
