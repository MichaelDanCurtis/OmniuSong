# Precise note editing

October 15, 2026. Implemented in the local development studio.

Select an existing note to inspect it, then change its pitch, start, length, or velocity. Empty cells still create notes on the eighth-note grid. Selecting an existing note no longer deletes it. Use **Delete note** for an explicit removal.

The **Selected note** menu lists every note in the track, including repeated notes that fall in one grid cell. Clicking that cell again cycles through those notes. Choosing a note follows its bar page and pitch range. Locked tracks allow inspection and disable editing.

## Exact values and saving

- Pitch is an integer MIDI note number, 0–127. The inspector also shows its musical name.
- Start is an integer tick, counted from zero. Length is an integer number of ticks. One quarter-note beat is 960 ticks; a 4/4 bar is 3,840 ticks. The beat display is informational and never replaces the exact saved ticks.
- Velocity retains the full unsigned 16-bit range, 0–65,535, including a MIDI 2 Note On with zero velocity. The percentage display is informational.

Press Enter or choose **Save note** to commit the edit. Multiple pending fields commit together. Changing the selected note, track, or session, or using transport, generation, Undo, or export controls, also commits pending note fields first. Invalid edits stop that action for review. Draft fields survive incidental studio redraws; leaving one field for another does not discard them. Valid edits use normal local autosave and revision checks. An unfinished field triggers the browser's leave-page warning. Invalid values are rejected and the field returns to the saved note value. Editing stops the transport and finalizes held input first, so an old recording reference cannot overwrite the edited note.

Moving a note beyond the loop extends the session to include its end, up to 128 bars. Restore and Undo do not automatically shorten a session. The grid shows note starts; length is controlled numerically. Drag handles, multi-note selection, quantization, and expression-curve drawing remain follow-on work.

## Original performance and expression

The first edit stores a versioned `editSource` snapshot of the note's original pitch, start, length, velocity, and expression. Later edits retain the same snapshot. **Restore original** restores those musical values and removes the snapshot. This survives save/reload and complete project backups.

Input identity, source address, receipt time, physical key duration, release velocity, end reason, raw MIDI/UMP capture, model estimates, and unknown external metadata remain unchanged. Moving a note shifts expression with it because expression ticks remain relative to the note. Changing pitch does not rewrite its original model estimates.

Shortening a note past its last expression point is rejected with the minimum permitted length. No expression point is silently dropped or rescaled. The current Web Audio preview still does not perform these saved expression curves.

Each edit adds a compact before/after journal entry. Explicit deletion retains the removed note in that journal. **Undo** restores the previous whole-project snapshot in this browser tab; its 40-entry stack is not durable across reloads. The journal is preserved in the project but is not yet a general undo browser. Projects retain the existing 5,000-entry history and archive size limits. **Clear notes** retains its earlier tab-local Undo behavior and does not create per-note deletion records.

## Verification

The saved **Note editing qualification** session contains two synthetic expressive notes, a locked source track, and 23 actual Basic Pitch estimates copied from the prior Magenta qualification. The synthetic input metadata is a fixture, not a physical controller recording.

Browser checks covered repeat-note selection, rejecting 699 ticks when expression reaches tick 700, exact edits to pitch 65/start 3,957/length 1,201/velocity 51,312, save/reload, persistent original restoration, deletion/Undo, locked inspection, editing/restoring an actual transcription, and bar-page navigation on Enter and Undo, MIDI 2 zero velocity, and atomic pending edits when changing tracks. A rejected shortening blocks track selection and preserves the original note. Widths 1,440 and 680 showed no horizontal page overflow; no console errors were captured.

The first completed browser backup was found in Downloads after the automation download-event wait timed out. The final backup was verified through its success message and downloaded file. The actual 2,455,739-byte file restored into an empty temporary store with all tracks, original snapshots, journal, raw capture, and both audio assets exact. The original Magenta session was unchanged.

An independent Mido parser read 26 MIDI 1 note-ons, with the edited note at tick 3,957 and its end at 5,158. MIDI 1 uses the upper seven velocity bits, producing 100 from 51,312; it omits expression with a warning. The separate OmniuSong UMP JSON trace preserves velocity 51,312 and the moved expression value at tick 4,657. This trace is not a standard MIDI 2 Clip File or a native transport test.

Evidence: [receipt](verification/note-editing.json), [source](verification/note-editing-source.json), [saved project](verification/note-editing-project.json), [MIDI 1](verification/note-editing.mid), [UMP trace](verification/note-editing.ump.json), and [studio screenshot](verification/note-editing.png). The full suite has 77 passing tests and no skips.
