# Native note editor UI — next packet

October 20, 2026. **Design and acceptance contract; implementation source prepared, integrated qualification pending.** This packet exposes the existing [native note-edit operations](NATIVE-NOTE-EDITING.md) through the native application. Library and archive evidence does not establish a working editor in any packaged binary. The coordinator owns shared integration, dependencies, builds and actual UI operation. The delegated implementation owns the app-local `note_ui.rs` module and this packet's `main.rs` integration.

## Smallest musician-facing scope

Edit one note on an existing unlocked `kind: notes` track. Use the current scrollable piano roll and virtual note list for selection, with an adjacent inspector. Provide **Save note**, **Discard note**, **Add note**, **Delete selected note**, **Restore original**, and the existing saved-change Undo/Redo. No new-track operation or backend contract is required: submit existing `Command::Edit` requests through the [session service](NATIVE-SESSION-SERVICE.md).

The inspector edits these exact integer values:

| Field | Bounds and display |
| --- | --- |
| Pitch | 0–127; also show the corresponding note name |
| Start, in ticks | 0–1,000,000,000 |
| Length, in ticks | 1–100,000,000; checked end arithmetic |
| Velocity, 16-bit | 0–65,535, including zero; no percentage or MIDI 1 conversion |

Retain bounded raw text while the user types, including empty or invalid intermediate input. Do not clamp, snap or quantize. Invalid values keep the draft and show a field error. Save is unavailable until valid; an unchanged complete result must not manufacture a history step. Exact expression-tail validation remains authoritative on the worker.

Default to **Keep current playback loop** (`PreserveLoop`). **Extend song to include note** selects `ExtendLoop`; crossing schema 1's 128-bar limit additionally requires an explicit, initially unchecked native-format upgrade choice. Never silently shorten the loop or promote the format. Disclose notes outside the current playback loop. The roll displays saved values until a matching acknowledged result is installed; draft fields do not trigger audio preview.

Add creates a local, unsaved note draft with one fresh strict note UUID, retained through Save/retry. It attributes no physical performance or AI provenance. Discarding that draft sends no Delete operation. Delete and Restore act on the exact selected saved note after resolving any draft. Restore retains the immutable `editSource`; snapshot presence alone is labelled **Original retained**, not proof that the note is currently modified. A no-op Restore may be rejected without changing history.

Expression-curve editing, quantization, drag/resize editing and performance capture remain required later. This packet makes no expressive MIDI playback, hardware, listening, latency or Windows acceptance claim.

## Exact selection and one draft family

Selection is `{project_id, DocumentNoteAddress {track_id, note_id}}`. Preserve exact legacy strings, including empty, Unicode and control-bearing keys. Never trim, normalize or convert them to strict transport `TrackId`/`NoteId`. Escape and truncate IDs only for display. Row indices and pitch are presentation aids, never edit targets; overlapping or identical-pitch notes must remain individually selectable through the list.

`NoteEditor` owns selection, a bounded existing/new note draft, its base project/revision, timeline choices and a checked draft serial. Rendering returns an app-local action request; it never opens storage, mutates the document or calls pure full-document mutation methods in paint. Main owns admission and guards. No full document or source snapshot is cloned into UI action state.

Allow **one active editable draft family: session name/BPM or one note**. Beginning the other family requires **Save / Discard / Cancel** for the active draft. Read-only saved-note inspection may preserve a metadata draft, but note inputs remain disabled until that draft is resolved. When a note draft is dirty, changing its selected note/track also requires resolution. Metadata Save/Discard controls must never save or discard a note implicitly. There is no compound atomic “save everything” claim.

Save commits the active family once, then continues the retained requested action only after acknowledgement. Discard resets only that family before continuing. Cancel preserves draft and selection but cancels the deferred action. Invalid drafts keep the dialog open; Discard and Cancel remain usable. Add/Delete/Restore choices retain their identities while awaiting resolution. Bind expected revision on their first actual dispatch after any preceding Save; never change it on a retry.

On a same-project document installation, re-resolve selection by exact identity. Delete clears a missing note instead of selecting a neighbouring array entry. Undo can reselect the restored identity when present. Opening a different project clears selection only after draft resolution. A replay may return a later document in which the target changed or disappeared; display that saved reality rather than presenting old attempted values as current.

## Stable intent, failures and completion context

Generate one operation UUID for an explicit submission and retain the complete `ProjectEditIntent`: project/address, values, policy and expected revision. Channel rejection or uncertain acknowledgement must not create another operation, alter the request, automatically retry or rebase it.

Each transport attempt separately captures its **current UI completion context**: selected project/revision, metadata draft and note selection/draft serial. A matching request ID is necessary but insufficient to install a result; a changed local context must preserve the newer draft. Freeze editable fields during admission while keeping paint and close responsive.

An exact retry after reopening the same project may have an older expected revision than the displayed document. Submit that old intent unchanged while capturing the current completion context for this new attempt. Do not require equality with the failed attempt's old displayed revision. Existing replay-first storage and the service's current-document reply resolve a lost acknowledgement without overwriting later saved work.

Only typed `EditRejected` or `RevisionConflict` with `outcome_unknown=false` proves an admitted edit was not published. A first channel rejection proves that attempt was not admitted; rejection of a retry cannot change the earlier outcome classification. Keep the draft and show the reason. After a conflict, offer explicit **review latest / make a new choice**; a later user-confirmed Save gets a new operation ID against the reviewed revision. If the old target is absent or no longer editable, **Release rejected note choice** explicitly keeps the most recent rejected intent and values as read-only evidence and permits a new target choice. No automatic merging or replacement request follows from the error text.

For unknown outcomes, retain one full intent and draft in a visible card. Explicit **Keep draft locally and inspect saved session** suspends it and releases navigation, avoiding a permanent guard trap. Reconnect opens a new worker generation and initializes only; retry remains explicit. Fresh note mutations stay disabled while that uncertain intent is unresolved. The retained card is window state, not a durable recovery inbox; closing does not cancel a possibly committed edit. Do not silently evict an unresolved card to admit another.

## Guards and bounded source display

Use separate `metadata_dirty()` and `note_dirty()` checks plus a combined unsaved-state check. Feed the combined check into Open/Create/Import/Undo/Redo, archive export/restore, audio start and capture attachment decisions. Library Refresh and retained-capture Scan/Inspect may remain read-only exemptions that never install a project or discard a draft.

Preserve [audio](NATIVE-AUDIO-ENGINE.md), live-audition and [recovery](NATIVE-CAPTURE-RECOVERY.md) guards. Playback/live/capture/finalization and unresolved attachment states disable note mutations. A dirty note prevents starting audio; typing note fields must never sound computer keys. Stop/Panic and responsive close remain available.

Dirty close protection applies **even while a read-only worker request is busy**. The current busy-Close shortcut and OS-close `!busy` condition must not bypass an unsent draft. Offer explicit draft discard/close-without-waiting versus keeping the window open. Never join, wait for filesystem work, or promise rollback/completion on close.

Remove full expression JSON formatting from paint. Borrow known fields from the existing prepared document: four musical values, bounded input source/protocol/group/channel, original release/key-duration metadata, and snapshot values. Display at most eight expression lanes and 64 points total, with counts and an explicit omitted-data notice. Preserve absent/null/empty-array distinctions; do not claim equality of undisplayed expression. Unknown model/snapshot fields remain retained without generic serialization. Raw session MIDI/UMP is not linked to a note merely by matching tick or pitch. Recording/audio tracks' opaque note-shaped metadata is never offered for editing.

## Acceptance before claiming completion

Headless checks must exercise real egui actions plus injected worker completions and prove:

- Exact selection across duplicate pitches, cross-track IDs, empty/Unicode/control keys, reorder, Delete/Undo and latest replay; missing targets never retarget by index.
- Integer boundaries, velocity zero, invalid intermediate text, expression-tail rejection, no-op behavior, locked/non-note tracks and bounded large-source rendering without full JSON serialization.
- Both draft families and Save/Discard/Cancel for selection, navigation, history, archive and recovery attachment; a held read-only worker cannot silently discard a dirty draft on Close.
- First-dispatch revision binding after a preceding Save, byte-identical retry with fresh transport IDs, explicit conflict/new-choice handling, suspension without navigation deadlock, and no automatic retry on reconnect.
- A stale completion or replay cannot erase newer local note fields; audio/recovery guards remain effective and note text never emits live notes.

The coordinator then uses a **fresh synthetic project only**, recording binary identity, AX/screenshot evidence and backend comparisons: select an expressive note; Save changed pitch/timing/zero velocity; cancel a dirty selection change; exercise Save/Discard continuation; Add/Delete/Restore; Undo/Redo; export/inspect/restore a copy; close, verify process exit, reopen and compare exact saved fields/source/snapshot/raw UMP. Verify original projects and source evidence unchanged. Include a visible out-of-loop note and explicit loop policy. Actual pointer/keyboard focus, field/action accessibility and readable light/dark layouts require observation; headless AX node presence alone is insufficient.

Until those gates pass, the source candidate does not establish a qualified packaged note editor or completed milestone 2.
