# Native note editor UI — independent review

October 20, 2026. Scope: the [UI contract](../contracts/NATIVE-NOTE-EDITOR-UI.md), app-local `note_ui.rs` and its `main.rs` integration over the existing qualified note-edit service. This review owns only this document and `crates/omniusong-app/src/note_editor_review.rs`. It does not change the musical schema, backend operations, archive rules or device code.

## Current disposition

The final reviewed implementation passes the coordinator's workspace gate: **394 passed, zero failed, two ignored private helper entry points across 41 result groups**, including 78 app tests and all 16 independent editor cases. Formatting, warnings-denied Clippy, the Mac development build and signature checks also pass. No remaining production blocker was identified in the frozen source. This accepts the bounded source/headless/preparation workflow; **actual packaged-editor acceptance remains pending**. The reviewer independently hashed the artifacts and read the central execution logs without running Cargo or operating the application.

The existing `2dd08d500b22333a315b642a541e8a36d05c3cc5466863e58e31a7ad589d35bc` bundle's previous native audition evidence does not qualify this new editor. Actual editing must use a newly built identified binary and a fresh synthetic session. No native GUI, audio device, microphone, live catalog, Cargo command or external service was used in this review lane.

## Corrections required during review

| Finding | Required behavior and source disposition |
| --- | --- |
| Making a new choice after a known conflict removed the retained failure before fallible target resolution. A subsequently deleted note or locked/missing track could lose its local values. | `NoteEditor::new_choice` now builds and validates the replacement before clearing the failure. A missing target returns an error with the old full intent and draft intact. The independent vanished-target regression checks this. The later explicit Release Rejected action is limited to a known rejected, suspended choice after review of its exact current project/revision; it retains the most recent full rejected intent and values as read-only evidence without submitting a mutation. An unknown outcome cannot use it. |
| A stale successful completion retained the newer active draft alongside the older attempted intent, misrepresenting what the old operation submitted. | `StudioApp::complete` now records `Active.note_context.draft` as attempted evidence, preserves the current active draft, and returns before document installation or deferred navigation. The independent test injects a changed local field after admission. |
| The generic last-action retry slot is cleared by unrelated successful operations. It cannot be the only home of an uncertain note edit. | `NoteEditor::failure` retains the complete intent and attempted draft independently. Suspension allows navigation while disabling fresh note mutation; exact retry uses the old intent with a newly captured current completion context. |
| A local failure to enqueue a retry does not prove that its earlier accepted attempt failed to commit. | `NoteEditor::not_admitted` retains an existing attempt's uncertainty classification. A first request rejected before admission is identified separately. |
| An old Save-and-continue action could survive explicit suspension or conflict review and later navigate unexpectedly. | Suspension and Review Latest clear the pending continuation; an exact retry keeps a continuation only while the unsuspended original UI context still matches. Cancel must cancel navigation, preserving local values and selection. |
| An unsent note draft or suspended uncertain card could be lost through the earlier busy-Close shortcut. | The combined unsaved guard precedes that shortcut, native close events cancel close for dirty drafts, and a separate window-local-card warning survives suspension. Explicit close does not wait for or cancel storage work. |
| Positional widget identities can send a retained accessibility action to a different note after reorder, or to a copied project reusing the same track/note keys. | Track selectors, inspector controls, list rows and piano hits now use explicit project-scoped identities. List/selector/inspector child scopes use `UiBuilder::scope_id`, which avoids egui's positional child salt. The inspector uses `Option<note_id>`, distinguishing no selection from a valid empty legacy key. The owner's real egui regression passes reorder and copied-project stale-target cases. |

The contract now explicitly says Cancel preserves draft and selection while cancelling the deferred action. Its first-enqueue distinction also states that rejecting a retry cannot change the earlier outcome classification. These requested clarifications match the inspected implementation.

## Architecture checked

- Durable targeting uses the complete project ID and exact `DocumentNoteAddress`. Legacy empty, Unicode and control-bearing keys are escaped only for display. A missing saved identity becomes an absent selection with no draft; a retained identity permits Undo to find it again without selecting an array neighbour. Presentation indices do not become edit scopes.
- The draft contains bounded integer text and explicit loop policy; it does not clone the project, expression source or raw capture into action state. No full-project mutation runs during paint. Zero 16-bit velocity is preserved, invalid intermediate input is retained, and authoritative expression-tail/no-op validation stays on the worker.
- A request owns one complete typed intent. Reopening a later revision does not rebase an exact retry. Known conflict recovery loads the saved target before an explicit new-choice Save mints another operation. `EditRejected` and `RevisionConflict` are the only admitted completion errors treated as known unpublished when `outcome_unknown` is false.
- Each attempt records project/revision, metadata draft and exact note context. Admission disables central editing; changed-context completion is nevertheless checked defensively. Read-only refresh and capture inspection do not install a document or erase drafts.
- Session metadata and note drafts have distinct Save/Discard behavior. Add is local until Save and has one stable strict UUID; discarding an unsaved Add emits no Delete. Save-before-Delete binds the second operation's revision only after the Save acknowledgement.
- Audio/live/capture and unresolved attachment guards remain ahead of note admission. A dirty note disables audio start. Stop remains available. This review exercises an injected engine port, not a native device.
- Source details borrow known values, display at most eight expression lanes and 64 points, and disclose omissions. Opaque note/model fields are not serialized into the inspector. Audio and recording tracks do not expose their note-shaped opaque metadata as editable notes.

## Independent regression source

`crates/omniusong-app/src/note_editor_review.rs` contains 16 cases using in-memory `SessionService::test_pair`, synthetic `DocumentView` replies, real egui actions/native-close input, and the existing injected audio port. They require no filesystem or device fixture. Its tested SHA-256 is `57769c654c443b5e9e7cdb6f57d976660d8c0d3ab7a658e01c9b5acddabccbfc`.

Coverage includes exact legacy/reordered/repeated-pitch identity, Delete/Undo selection, integer rejection and velocity zero, retained vanished-target evidence, stale successful completion, suspended uncertain navigation and same-byte retry at a newer displayed revision, conflict review/new choice, busy-read Close and native close event, Save-before-Delete revision binding, Add retry identity, uncertainty after local queue rejection, suspended-card Close, Cancel continuation, audio/recovery guards, locked/non-note scope rejection, and bounded expression/opaque-source rendering.

The central log `.runtime/rust-note-ui/app-fourth-tests.log` reports 78 passed, zero failed and zero ignored; its SHA-256 is `776ea86f507247275a85c77afa01fa3cf4b0131c6e6e4e9c78bdcf22c0a78d56`. The reviewed/tested production hashes are `main.rs` = `a4ebd6a8c46786deaab7a43ee61b16405522755bf5543727612ce2e20c68f5d3` and `note_ui.rs` = `7fc5358680c5d34e4bbfcda8b378a8e76bb4e8014b7d7bcbc3cd11e79571ae62`; the adopted UI contract hash is `e927ea49ee268cf0011dc80eceee09c3cd29e62a75a03025689c7b0e694cc1a9`. The coordinator also reports `fmt-initial.log` exit zero; the retained log is empty (SHA-256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).

Earlier central attempts exposed fixture issues: egui 0.37 uses `all_styles_mut`; returned font texture deltas must be consumed; static accessibility text appears as both a Label and its TextRun child; the actual button is “Stop audio”; and ordinary playback Stop is queued behind StartPlayback acknowledgement rather than sent on the live urgent-ticket path. The final independent test counts Label nodes once while inspecting all node text for opaque leakage, and proves the queued playback Stop is sent exactly once after the synthetic Playing acknowledgement. Those corrections preserve the intended assertions. The separate real widget-identity failure required the production scoping correction above.

Actual pointer/text focus, accessibility, visual readability, Save/Discard/Cancel, Add/Delete/Restore, Undo/Redo, archive copy and restart preservation remain separate packaged-UI gates. These tests do not establish expression editing, capture, expressive MIDI playback, latency, listening quality or completion of milestone 2.

## Final source, build and preparation evidence

The reviewer independently checked all **104 source snapshots**, all six logs and the binary in [rust-native-note-editor-tests.json](../verification/rust-native-note-editor-tests.json): every recorded length and SHA-256 matches. Parsing `.runtime/rust-note-ui/workspace-final-tests.log` independently yields 41 successful result groups, 394 passed, zero failed and two ignored helper entry points. The final run includes the same 78 app cases. Its SHA-256 is `392c8c278ea8d227a277acb72e19765c2e5ff0cfe7a8500fdfc3129640234c60`. The retained Clippy log finishes successfully, the build log records a completed build and valid plist, and the signature log reports valid-on-disk/designated-requirement success. The empty final format log and coordinator exit-zero result are recorded together in the receipt.

The final production hashes supersede the earlier app-only checkpoint above: `main.rs` = `8b4af21326f5463fb2991fc1f4d38c647ffefb7b38ae2e359e984efa5de07649`; `note_ui.rs` = `13c3a8a42e44ef8408eee052be7a1d98cb3422d9dac395e2e3ba65715fe94ac3`. The coordinator reports only the two mechanical Clippy corrections after the earlier 78-case gate; the complete final workspace gate covers these current bytes. The independent test file remains `57769c654c443b5e9e7cdb6f57d976660d8c0d3ab7a658e01c9b5acddabccbfc`.

The new `dist/OmniuSong Native.app/Contents/MacOS/OmniuSongNative` binary is **85,287,456 bytes**, SHA-256 **`85ef6cd710eb98aa391cac9468d262c2b34adb02e0305e98b310e602ffa02162`**. The five unchanged vendor callback tests are inherited evidence explicitly marked not rerun; they are not counted as fresh editor qualification.

The [UI receipt](../verification/rust-native-note-editor-ui.json) accurately reports `completed=false`, `studioOpened=false` and `nativeEditorOperated=false`. The retained loading AX text contains the responsive opening screen and Close control. The matched sample identifies PID 63138 and SQLite `openDatabase` → macOS `__open`; that stack does **not** prove that a permission prompt exists or establish the cause of the wait. All five recorded loading artifacts match their hashes. No actual editor, input or microphone operation is asserted, and the older `2dd…` live-audition walkthrough is not transferred to `85ef…`.

The fixture receipt and source JSON also match. The only new saved project is **`ca0f6e4f-c40a-4d7b-a435-77d3a0ddbd19`**, named “SYNTHETIC note editor walkthrough 5fa86e22”, revision zero: two tracks, six synthetic notes. Four unmuted procedural notes have no expression; the muted track retains the expressive legacy-key note, raw synthetic UMP and the out-of-loop 129-bar example in its original four-bar schema-1 document. Source JSON is 6,923 bytes, SHA-256 `f20554424e51d664c6a2cfd67c43ccb786324bd335fa2fde34b454b0ead0290b`. The 114,688-byte complete source archive matches SHA-256 `6ccd987a4c4a16d6c3587fdf4a845a27bee3c9b83f63a57cf3cfe2c42b6b4c06`; its SQLite header identifies OSAR version 2. This is a prepared synthetic fixture, not a captured performance or completed editor walkthrough.

The render preflight supplies that **same source JSON twice**. Both independently checked WAV headers describe IEEE float32 stereo at 48 kHz; each file is 675,060 bytes with 675,016 PCM bytes representing **84,377 frames**. Both whole-file hashes are `0e6e9e80c892c71b4e091bce9c6d5787d5ed1edfad2ea07a0135a5cca0286d90`, and both PCM hashes are `967d6b42aa7df63f11106d620023eed964d0dadb2009ebe3e2d6ead71597841e`. The receipt correctly reports no differing samples, `pcmDifferent=false` and `projectCommitVerified=false`. This verifies a deterministic production-render path preflight; it does not prove that an actual saved UI edit changes audio, physical playback or listening quality.

For [preservation](../verification/rust-native-note-editor-preservation.json), the reviewer verified the three snapshot-file hashes, then compared their table columns and row-hash sets. All ten prior project rows remain, exactly one synthetic project row is added, and the other nine tables are unchanged. Every table and the catalog file hash agree between the after-fixture and after-loading snapshots. All three owned immutable take files were independently rehashed successfully. All **19 original prototype project files plus 11 asset files** independently match [rust-migration-baseline.json](../verification/rust-migration-baseline.json), whose own SHA-256 is `1169fdd5337f98798b0dbd6fb2b87c69ff5f8ab6759ad7454f4c587e3b25536e`. This audit compared retained SQL row snapshots and current original-file bytes; it did not open or repair the live catalog. Integrity/foreign-key checks are coordinator-recorded evidence.

No inconsistency requiring a source or receipt correction was found in this final audit. The remaining gate is the explicitly pending new-binary synthetic editor interaction, actual changed-note render agreement, edited archive/copy and clean restart/preservation walkthrough.

## Actual 85ef packaged walkthrough — independent artifact audit

October 20, 2026. **The bounded actual-editor gate is accepted.** This appended
audit supersedes the pending-UI disposition above; those earlier paragraphs
retain the historical loading/preflight boundary. The version-2 [UI
receipt](../verification/rust-native-note-editor-ui.json) and
[preservation receipt](../verification/rust-native-note-editor-preservation.json)
now describe the completed synthetic walkthrough. No discrepancy requiring a
receipt or production correction was found. This lane inspected retained files,
screenshots, read-only SQLite views and process information; it did not operate
the app, rebuild, run Cargo, open a device or mutate a catalog.

All **119 distinct fingerprinted files** directly referenced by those two receipts match
their recorded byte lengths and SHA-256 values, including 47 AX transcripts and
47 screenshots, both complete archives, source/render evidence, the render
currently retained helper binary and the packaged executable. The two additional
WAVs referenced inside the later recheck receipt also match. The executable remains 85,287,456
bytes with SHA-256
`85ef6cd710eb98aa391cac9468d262c2b34adb02e0305e98b310e602ffa02162`.
All 104 source fingerprints in the earlier build receipt independently match
committed **`e1e8d9d31c48170dfdfc831ff60a1d73c52c6190`**. The render preparer,
callback cursor and render-helper source also match that commit; both fixture
and render receipts' embedded source hashes match their actual example sources.
The newer, uncommitted clip-edit backend is separate software work and is
**not** attributed to this unchanged bundle.

The five retained project snapshots independently match these identities:

| Snapshot | Revision / schema / bars | UTF-8 bytes | SHA-256 prefix |
| --- | --- | --- | --- |
| Synthetic source `ca0f6e4f…`, r0 | 0 / 1 / 4 | 6,923 | `f20554424e51` |
| UI-committed source r1 | 1 / 1 / 4 | 7,136 | `aafcf00b9c7c` |
| Source r15 | 15 / 2 / 129 | 7,766 | `092f27781322` |
| Restored copy `e13c17b4…`, r15 | 15 / 2 / 129 | 7,766 | `e2d08af2d7ff` |
| Renamed/reopened copy r16 | 16 / 2 / 129 | 7,762 | `640e7b20976a` |

The 393,216-byte edited OSARv2 archive matches
`cef313896ce14d894124349a5a8a27a08b4f5a1887d03b7871512c0840153f74`.
Its project payload is byte-identical to the r15 JSON. It contains nine contiguous
history rows, fifteen operation rows and no audio assets/takes. Every retained
history before/after checksum and operation request/result checksum passes.
Revision 1's durable operation result is byte-identical to the r1 render input;
history sequence 1's before-image is byte-identical to r0. This supplies the
commit binding that the standalone render helper deliberately does not claim.

The operation results independently establish the observed sequence: initial
four-value Save with velocity zero; Undo/Redo; Restore; exact Delete/Undo; authored
Add; corrected expressive-note Save/Restore; out-of-loop Preserve; explicit
schema upgrade to 129 bars; schema Undo/Redo; Restore without shrinking; and the
final authored-note velocity Save. The two recorded rejected operation IDs are
absent from the durable ledger. Their rejection/retry/review screens retain the
expected revision and ID; no duplicate admission is inferred from an unchanged
error screen alone. Known semantic rejection is covered here; uncertain-outcome
and true concurrent revision-conflict GUI workflows remain separately pending.

The copy-r15 document differs from source r15 only at its owned root ID. The r16
copy adds only the explicit root name/revision change. All fifteen copy operation
mappings and all nine copied history rows retain exact source content after
normalizing only their owned project/operation IDs. The source has nine history
rows/fifteen operations; the renamed copy has ten/sixteen. Comparisons preserve
raw unfamiliar numeric tokens, expression, immutable first snapshots, input
release/key-duration metadata and synthetic UMP across all 33 archived
history/result snapshots. No packet-to-note or physical timing claim is made.

Both render WAVs independently parse as IEEE float32 stereo at 48 kHz, 84,377
frames, 675,016 PCM bytes and 675,060 total bytes. Their PCM hashes match
`967d6b42aa7df63f11106d620023eed964d0dadb2009ebe3e2d6ead71597841e`
and `a401df68ca0b80b88e27292f8229b981e8c3a8bb273c7310a83c9341039a2dc4`.
An independent byte comparison reproduces **49,658 changed samples across
24,829 frames, first changed frame 3,095**, with no unmatched-length tail.
The r1 note changed from 60/123/987/51313 to 61/137/1001/0, with a matching first
snapshot and unchanged expression. The resolver catalog still matches its
helper-recorded unchanged hash. This accepts actual saved-edit/offline production
render agreement, not listening or realtime sample delivery.

The initial render helper was subsequently relinked by a central test build, so
the currently retained executable hash alone cannot identify the helper used by
the initial run. The coordinator corrected this evidence boundary with a fresh
recheck: `saved-r1-render-recheck-identity.json` records the helper before and
after as **9,409,896 bytes**, SHA-256
`b6bf8ef2787a5b2db464efc9a0a003b839e696735fb3ab85735888e616775686`,
and exit zero. The identity file, recheck JSON and stdout log independently match
their updated receipt fingerprints. Both new WAVs are byte-for-byte identical to
the initial WAVs, and input identities, document facts and frame differences
match. This recheck uses current clip-backend dependencies with unchanged
renderer/helper sources; it is separate offline evidence and does not replace
or extend the `85ef…` packaged binary's implementation scope.

Preservation checks independently retain **all 67 baseline rows across all ten
catalog tables**, including the ten prior projects and their closure. Fixture
creation adds only the allowed source project; fixture-to-loading table and
catalog-file hashes remain equal. The final live catalog, opened query-only,
matches every after-walkthrough table hash and its complete file hash, and passes
integrity/foreign-key checks. Only the two allowed synthetic project IDs are
added. All three owned take files and all **19 prototype project plus 11 asset
files** rehash exactly against their recorded baselines.

Representative screenshots and AX transcripts agree on the coincident-note
choices, saved four-field values, invalid draft/disabled Save, Save/Discard/Cancel
dialog, retained expression-tail rejection, explicit upgrade, archive inspection
and reopened source/copy. The pointer-focused velocity field visibly contains
49123 while its saved roll still shows 65535; r15 subsequently commits 49123.
The earlier direct AX-click/typing attempt left 65535 unchanged without focus.
Thus pointer focus plus keyboard entry passed; a full keyboard/AX focus workflow
did not. Expanded details retain escaped legacy keys and original expression
values without displaying opaque source objects.

Process close/relaunch is coordinator-recorded as initial **63138/70220**,
intermediate **73322/73327**, explicit close of that pair with no native process
remaining, then restart **74019/74024** from the same executable. The audit's
read-only process inventory corroborates the older IDs absent and the new GUI /
child pair present at that path; it cannot recreate the historical no-process
interval. Reopened AX and exact catalog payloads bind source r15 and copy r16.
The output-only follow-up reports MacBook Pro Speakers, 48 kHz stereo,
`playback_finished`, **100,478 consumed frames / zero reported xruns**, followed
by Stop and released editing guards. No captured physical sound or microphone
operation follows from those counters.

Acceptance remains bounded: the light-theme screenshots show weak contrast for
gold **Unsaved edits** text; H07 is not fully accepted. Full keyboard navigation,
IME, scaling, healthy live focus-loss and uncertain/conflict GUI paths remain
open. Current native audio is one-shot full-content rendering, not demonstrated
looping. Expression playback/editing, physical MIDI/MPE/input, native inference,
plugins, long recording/power-loss qualification, listening, measured latency,
installers and Windows remain outside this gate. This does not complete milestone
2 or release acceptance.
