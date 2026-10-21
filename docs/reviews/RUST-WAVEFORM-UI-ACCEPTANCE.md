# Native waveform UI acceptance audit

Independent evidence audit on 2026-10-20, after the coordinator froze the final
receipts. **Accept the bounded Mac source-waveform UI slice on binary
`9b2fb28c5342bd1c272d8e7a495741fdc4bbdd77fbf43c61f64362ba93eca4a0`
(87,964,192 bytes).** No blocking source/evidence mismatch remains within the
scope below. This does not close the full workstation or release requirements.

This reviewer inspected retained AX text and PNGs, hashed files and compared
read-only evidence. No Cargo, formatter, native GUI, device, live catalog SQL or
mutation was performed. Existing architecture/source/initial-evidence reviews
remain frozen; only this acceptance document was written.

## Exact source, gate and executable boundary

All **120** source entries in `gate2-run.json` match current bytes and SHA-256.
The final source receipt contains the identical source list. All six bound logs
also match: format check, offline workspace/all-feature tests, offline
all-target/all-feature Clippy with warnings denied, offline all-target build,
Mac packaging and strict deep signature verification have recorded exit code 0.
Independent summation of the 45 test result groups confirms **517 passed,
0 failed, 2 ignored**. The source receipt reports 128 app tests, including 16
waveform UI tests; 14 waveform backend/client tests; six waveform DTO tests;
and 12 independent store-reader tests. The ignored cases are subprocess helper
entrypoints, not hardware qualification.

The current packaged executable and Info.plist were independently rehashed and
match the final receipts. The executable hash also matches the recorded
pre-restart process-exit receipt. The final source receipt correctly preserves
`correctedPackageNativeUiQualifiedAtSourceGate: false`: source gating preceded
the actual walkthrough. Its later-UI path points to the final UI receipt without
a cyclic hash; the UI receipt binds the final source-receipt hash. That temporal
separation is internally consistent.

Gate 1's 516-test source and `1b7076…` package remain failed *UI* acceptance
history because the lower waveform lane was clipped. Gate 2 includes the
measured full-row viewport correction and a real nested-layout geometry test.
The final source reserves Small-text height plus item spacing plus the 78-point
plot, keeps the 340-point maximum and channel virtualization. This changes
layout, not decoded data or draft semantics. The implemented UI/qualifier asks
for **1,024 bins** per channel, within the adopted 1–1,024 bound; an earlier
512-bin design proposal is not the current default.

## Actual walkthrough evidence

All **59** artifacts bound by the frozen final UI receipt match their SHA-256
and lengths. The 25 named steps each have distinct PNG and AX-text paths. The
following conclusions were checked against the retained artifacts rather than
inferred from a successful compile:

| Steps | Supported observation |
| --- | --- |
| 03–04 | Explicit legacy Show displays the authenticated full WAV `[0, 24000)`, saved `[3000, 21000)` and original `[6000, 18000)`; the full positive and negative lane is visible. |
| 05–07 | Use-default trim-end gives only a local `[3000, 24000)` preview. Hide clears the plot while retaining the dirty draft; a new explicit Show recovers that same draft preview. |
| 08 | Native source `[100, 24100)` is distinct from saved `[1100, 10100)`, original `[6100, 18100)` and signed arrangement placement −480. |
| 09–11 | Step 09 did not change source start from 1100. The recorded coordinate-focused keyboard step 10 changes it to 3000; step 11 shows a 6,000-frame draft with 100/200 fades and placement −960, distinct from both saved and original settings. |
| 12–14 | Invalid text `bad` remains visible, direct Save is disabled and the local-preview explanation retains valid saved/original outlines. The dirty-navigation modal appears; Cancel preserves the invalid draft. |
| 15–17 | After explicit Discard, the raw-events track has no audio waveform. Exact second-clip selection requires Show and displays its own saved/original window and placement. |
| 18–19 | Opening the restored project clears the former view; explicit Show binds the copy's revision 18, not the source's revision 17. |
| 20–22 | Playback reports 48 kHz stereo, 25,600 frames and zero reported xruns while edit controls are disabled and Hide remains enabled. The next snapshot already reports natural completion at 47,000 frames, then Stop is observed. |
| 23–25 | A prepared copy view is shown, both processes are recorded as exited, and the unchanged executable is restarted. Copy revision 18 and source revision 17 each show a freshly prepared source view. |

Visual inspection of final04, final11 and final25 confirms a complete 78-point
positive/negative lane above the bottom status region. Final11 visibly contains
separate solid saved, dotted original and dashed local outlines. Their text
labels retain the exact source domains and arrangement units. This resolves the
specific initial clipping defect; it is not a claim about every screen size,
font, wide-channel layout or monitor.

The source input fields retain long stored legacy Number spelling while the
waveform labels explicitly use renderer-rounded estimates. Native coordinates
remain integer frames. No local draft was published in this walkthrough. The
source project remains revision 17 and the restored project revision 18.

Two action boundaries are material:

- Step 09's filename describes the attempted keyboard route, but the AX state
  still has source start 1100. Only the succeeding step 10 is accepted as the
  successful coordinate-focused keyboard entry, corroborated by its focused
  field value 3000 and updated local overlay.
- Step 20 proves **Hide enabled during playing**. Step 21's filename is not
  proof the Hide action completed while audio was still playing: its status
  already says `playback_finished`, 47,000 frames. The action's timing relative
  to natural end was not measured. This audit makes no stronger coexistence
  or in-flight cancellation claim.

The selected output is labelled MacBook Pro Speakers in the AX snapshots.
The evidence is a native output status/Stop observation, not listening evidence,
physical emission timing or latency measurement. The process-exit receipt
records pgrep exit code 1, no matches and both app/audio worker exited. This
reviewer did not issue a new process query.

## Source data and preservation

The final offline waveform report is byte-for-byte identical to the initial
673,101-byte report. Its SHA-256 is
`5456cf22948e053fcf0a04b4efe7fa575a86d35fbf5d93c1fd1d96398aab6622`.
The independent bin-audit receipt and script are unchanged and their hashes
match. The earlier independent evidence audit recomputed **3,072** exact integer
bin intervals and float32 extrema from the retained WAV and original SQLite
PCM, with saved/original settings bound to the exact revision-17 document.
Because the final report is byte-identical, that numerical result applies to
the final report without substituting a different computation.

There are three selected mono 48 kHz views, each covering the complete 24,000
source frames: WAV origin 0 and two views of the same native stream at origin
100. They are not three independent recordings. Raw event ordinals are
explicitly excluded from audio interpretation. These actual source fixtures
have no declared gaps or unknown tails; wide channels, gaps, recovered tails,
large origins and cancellation races remain software-test coverage.

All four preservation snapshots—before waveform work, after initial UI, before
final restart and after restart—are identical except capture timestamp. Their
fingerprints all match the final preservation receipt. They retain:

- **14 projects, 10 tables and all 220 rows**, including projects, edits,
  operations, exact imports and copy-lineage tables. The initial audit also
  recomputed all 220 row hashes from the isolated retained catalog; the final
  lists are identical to those verified lists.
- The same whole-catalog fingerprint: **7,757,824 bytes**, SHA-256
  `994f9cdd6b591363211972d22b47dbe82c3246c47da8317fc8068abeddcceff0`.
  This comparison uses captured catalog fingerprints; the reviewer did not
  query the live catalog.
- Four immutable take objects, all independently rehashed again against their
  recorded lengths and SHA-256.
- All **30 prototype files** in the unchanged baseline manifest: 19 project
  files and 11 assets, independently rehashed against path, size and SHA-256.

Row hashing retains SQLite JSON payloads as exact strings rather than parsing
and rewriting their numbers. The unchanged full catalog and exact row hashes
therefore cover opaque metadata, original snapshots, null/absence, history,
receipts and copy proofs. The read-only waveform work added no project,
revision, operation, history or owned take. No archive action was part of this
walkthrough; archive publication/fidelity remains separately qualified work.

## Scope retained as open

This accepts explicit source Show/Hide, the corrected visible lane, saved/
original/local overlays, invalid-draft fallback, selection/copy isolation,
bounded output-status coexistence and process restart/reprepare on this exact
Mac package. It does **not** accept:

- Actual cancellation while a native source read is in flight, nor an I/O
  deadline for stalled OS reads or the noninterruptible sealed scan.
- Native wide-channel/gap/unknown-tail or long-recording GUI qualification;
  these are bounded software paths, not part of this actual mono fixture.
- Sustained preparation/audio coexistence, listening or physical/latency tests.
- Native input or a human performance recording. No input was opened for this
  waveform walkthrough; software loopback restrictions are unchanged.
- Waveform dragging/zoom, arrangement manipulation, full recording/monitoring/
  overdub compensation, MIDI 2/MPE transport and Clip Files, plugins, native
  inference, installers, Windows or complete release acceptance.

The remaining limits in the final receipts are accurate. No additional action
is required to correct this bounded acceptance evidence.

## Final evidence fingerprints

The source and UI manifests below retain all 120 source and 59 artifact
fingerprints; every referenced entry was independently rehashed. Selected
artifacts are repeated here to make the acceptance boundary easy to inspect.

| Evidence | Bytes | SHA-256 |
| --- | ---: | --- |
| `docs/verification/rust-native-waveform-tests.json` | 29639 | `606642b3414d97e5a09640d1e57c3ced9384120dc929f4eba6e3c962d6f759f4` |
| `docs/verification/rust-native-waveform-ui.json` | 20170 | `d497bf0ceb013f1adab78cc0aa1feb1021a6ea2da9c8651a45e377d1d056d729` |
| `docs/verification/rust-native-waveform-preservation.json` | 2652 | `248af4e44beef01396efcf661c50359301fa4323bc61e70ead3a7a340bd3011c` |
| `.runtime/rust-waveform-ui/gate2-run.json` | 24667 | `a613868fd1cdaf0b54868a17ace144de6c27815e1e729558ab3045b20832f25f` |
| `.runtime/rust-waveform-ui/gate2-workspace.log` | 57345 | `30557b3e0810d8c9eb40a935cf8c65df31aae30f2101c87c87c0f5864bd021ff` |
| `.runtime/rust-waveform-ui/gate2-fmt.log` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `.runtime/rust-waveform-ui/gate2-clippy.log` | 174 | `a0543d7bbb908070a99477bf7f46fb5ab23655267394c85f0df6cd56de1788ff` |
| `.runtime/rust-waveform-ui/gate2-build.log` | 174 | `4cdcd6eedefff20fdced4bec45bd1efca9d9a2306937a825be34c5f0f8712c72` |
| `.runtime/rust-waveform-ui/gate2-package.log` | 479 | `5e83337ef9811f028243882656fc2d93735e92e27589ea789c7aedefac9e3913` |
| `.runtime/rust-waveform-ui/gate2-signature.log` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `dist/OmniuSong Native.app/Contents/MacOS/OmniuSongNative` | 87964192 | `9b2fb28c5342bd1c272d8e7a495741fdc4bbdd77fbf43c61f64362ba93eca4a0` |
| `dist/OmniuSong Native.app/Contents/Info.plist` | 785 | `1206796078276957cc3f5042561a9f2a5d26ce8a5c3d4682dc4e62127b4d2316` |
| `docs/contracts/NATIVE-SOURCE-WAVEFORM.md` | 6427 | `50e0491c6849396932eb4ecbebef79faf2b17a6caf86068d7b24b1c7bfa0122a` |
| `crates/omniusong-app/src/waveform_ui.rs` | 75084 | `0eafc0d039557c5a95bfce25d740ae6c6bde420cc13b31526a3fe4bcafc605ae` |
| `.runtime/rust-waveform-ui/offline-waveforms-final.json` | 673101 | `5456cf22948e053fcf0a04b4efe7fa575a86d35fbf5d93c1fd1d96398aab6622` |
| `.runtime/rust-waveform-ui/independent-bin-audit.json` | 2792 | `7e1b9eafe227ae5bc8246ad71a59f19e93bcb8f5111edfb7e9abe8f72738f454` |
| `.runtime/rust-waveform-ui/final-process-exit.json` | 248 | `61a91dde735c4ba84a0b5113fa9de05b20ecff994ad19e63c1b9ffd35214adb4` |
| `.runtime/rust-waveform-ui/after-restart-store.json` | 20587 | `4504825935b4e1d304e69c5df136f7df200c91eda114e8cfd7e31dd7219401c4` |
| `docs/verification/rust-migration-baseline.json` | 7170 | `1169fdd5337f98798b0dbd6fb2b87c69ff5f8ab6759ad7454f4c587e3b25536e` |
| `.runtime/rust-waveform-ui/final-ui/04-corrected-legacy-full-lane.png` | 258366 | `265a33aa1274d34f1af2c66831d5c2f8da10382ca9d3f7d6144b6a35650d533f` |
| `.runtime/rust-waveform-ui/final-ui/04-corrected-legacy-full-lane.txt` | 8896 | `24bbcc27286a7e5000a835fafc3816894b147a5635dfb88426d818912f48c61c` |
| `.runtime/rust-waveform-ui/final-ui/10-native-real-pointer-keyboard-window-preview.png` | 249682 | `b91fe596627e192c42d6c36576c0d5dd9f0afadca4ffa7f524de0c4adf169e52` |
| `.runtime/rust-waveform-ui/final-ui/10-native-real-pointer-keyboard-window-preview.txt` | 9333 | `97c5cae98efd1e5077b5d4d0182f6a0b3ce6b2d5d085579459b8b5f411783bcb` |
| `.runtime/rust-waveform-ui/final-ui/11-native-three-distinct-overlays.png` | 252499 | `65456e322f3038f592f4f54c20b5b672e770760739b885c1373c7de71b3f295b` |
| `.runtime/rust-waveform-ui/final-ui/11-native-three-distinct-overlays.txt` | 9288 | `e85f1e1d1821975609df975e4a7d8501ac6a36a0a095012ce340f1859ba65741` |
| `.runtime/rust-waveform-ui/final-ui/20-playback-disables-edits-keeps-hide-available.png` | 202545 | `fc8223b02f5ce8fb3db66e146fa93e4f3ff00ce9c244a075182ba4416bfe95d1` |
| `.runtime/rust-waveform-ui/final-ui/20-playback-disables-edits-keeps-hide-available.txt` | 9430 | `3ac237945ca0916bdf595cda83c66db58f998cce26ef6e0cc495098408cfc551` |
| `.runtime/rust-waveform-ui/final-ui/21-hide-during-saved-output.png` | 222711 | `55abbe9c3f878d88f8df40a5c95b0cce1e2dd496841196fd07f2277b5c41c993` |
| `.runtime/rust-waveform-ui/final-ui/21-hide-during-saved-output.txt` | 8068 | `6f24fd266e6e3aad15330d93a8268cee0aacd1f19653387aa843666c85214b70` |
| `.runtime/rust-waveform-ui/final-ui/25-restart-source-r17-full-lane-ready.png` | 244010 | `0242d34ba6059e7c0297dcb338456fd0f4387806a5c22459468de27a62217752` |
| `.runtime/rust-waveform-ui/final-ui/25-restart-source-r17-full-lane-ready.txt` | 8941 | `e85df2edee3360f655f0ca4668e9ddb95187fb19f70f1fb5d1eff8919a07a92a` |
