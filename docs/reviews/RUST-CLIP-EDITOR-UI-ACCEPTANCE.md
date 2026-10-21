# Native clip inspector: bounded Mac acceptance

Review date: 2026-10-20 Pacific / 2026-10-21 UTC. The recorded walkthrough and
its saved-state evidence support acceptance of this clip-inspector slice on the
exact `216d8e1c…` Mac package. No material evidence mismatch or acceptance blocker
was found. Milestone 2 and the full native-workstation goal remain open.

This review independently reads the frozen [UI receipt](../verification/rust-native-clip-editor-ui.json),
[preservation receipt](../verification/rust-native-clip-editor-preservation.json),
[source gate](../verification/rust-native-clip-editor-tests.json), saved JSON,
captured accessibility text/screenshots, and isolated SQLite backup/archive.
It rehashes files and recomputes saved-document, row, archive, copy-map and PCM
comparisons. It does not repeat the GUI session, run Cargo, access the live
catalog, open a device, or judge sound. SQLite inspection used read-only immutable
connections to copied evidence; the embedded owned journal was also examined in
memory. The coordinator's audit script was read, not rerun against its live-catalog
path.

## Frozen implementation and evidence

All **113 compiled-source snapshots** and **131 unique bound artifacts** match
their recorded byte lengths and SHA-256 values. The final test log independently
totals **469 passed, 0 failed, 2 ignored helper entrypoints in 45 result groups**,
including 112 headless app tests, 24 independent clip-UI tests and nine independent
range-rejection tests. The recorded final format, warnings-denied Clippy,
workspace build and package gates passed. This audit did not rerun them.

The executable is 87,413,920 bytes with SHA-256
`216d8e1c3760242f13985a727629752a7d753e0b16477ab275899d2ed54f1155`.
The earlier 85ef note-editor package and 425-test backend gate retain their
separate evidence boundaries. The [prior source review](RUST-CLIP-EDITOR-UI.md)
remains unchanged at 11,722 bytes,
`6dcde9e26f5903307f28a4a3a14a2b8c133fc6a52ea11a486deeaf906789eb83`.
Its statement that actual clip UI was then unqualified is historical, resolved
by this later receipt rather than by rewriting that review.

| Reviewed artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `docs/verification/rust-native-clip-editor-ui.json` | 35,867 | `676a9d8ec139f33ad8b98ba62ce310dea8df7a927920d33fc59dee850d48b709` |
| `docs/verification/rust-native-clip-editor-preservation.json` | 3,801 | `c54253fcac8a35640858b764672cbdaddba00f43f0ea14d95689685e3ad5e2af` |
| `docs/verification/rust-native-clip-editor-tests.json` | 26,373 | `e853780cab35aa3eaebc8ca75324a310d4cf7e829fd8918fa3ce3fbeb27fd3d4` |
| `.runtime/rust-clip-ui/committed-audit.json` | 2,418 | `2aa442d5d81a026e9094a8475b07059a1481519e648cc1fb2ef9c911f0c3b803` |
| `.runtime/rust-clip-ui/audit_walkthrough.py` | 7,681 | `7adf4073bf3bb2528dcfc2f09db7905f0762022ff3fc1438eb4b06b3e307cf80` |
| `.runtime/rust-clip-ui/catalog-after-clip-ui.sqlite3` | 7,757,824 | `f18cfbb07e71d0ec1922bc949e65d48842a0ef221b2566f3b5bd80856f6a363f` |
| `.runtime/rust-clip-ui/edited-session.omniusong-native` | 606,208 | `c2070e9cfebfb03057ddd7dd482f26e6615cf13d9b6eab87b51aa34a6c6e3bd3` |
| `.runtime/rust-clip-ui/copy-proof.json` | 4,140 | `87e436c19a2e3de0b793c5de8e346ad89d616e3123a2b5b227886fde76ab0a45` |

The complete CUA proof index has 35 distinct names and 62 distinct referenced
AX/screenshot files. Every referenced file exists and is fingerprint-bound.
Numeric step 2 appears twice (`invalid-integer`, `dirty-cancel-dialog`); these
are distinct complete artifacts, not an overwrite or missing step. The later
`update_delivery_docs.py` is unbound documentation-maintenance material and is
not used as qualification evidence.

## Saved musical state and recovery controls

Whole-document comparisons were independently reconstructed from baseline r5
through every saved r6–17, with tagged JSON numbers preserving numeric spelling
and type rather than floating-point conversion. Each document equals the
expected explicit edit, Undo/Redo or Restore transformation, including all
otherwise untouched root, track, source, clock and opaque fields.

- Legacy placement becomes 321 ticks at r6. Its first snapshot permanently
  retains placement 123 and the exact original seconds/fades. Later trims retain
  `0.062500000000000000000000000001`,
  `0.437500000000000000000000000001` and fade-in `0.0100000`; Use default removes
  fade-out. Restore r16 recovers the first values and spellings, keeping the
  snapshot; Undo r17 restores the edited settings.
- The first native clip at r7 uses source start 1100/count 9000, signed placement
  −480 frames at unchanged 48 kHz, and equal-power fades of 480/960 frames.
  Recording payload version advances to 2. Its first snapshot retains source
  6100/count 12000, placement 0 and explicit `fades:null`. Undo r8, Redo r9,
  Restore r10, fade removal r11 and Restore r12 have the exact expected whole
  documents. Null, omission and an authored fade object remain distinct.
- The second native clip is addressed independently: r14 places it at frame
  36000 with 11000 source frames and explicit zero linear fades. Its own first
  snapshot retains placement 12000, count 12000 and equal-power fades 100/200.
  The first clip and the muted raw-event track remain exact.
- IDs, source tuple, rate, range units, immutable source metadata, large integer,
  long decimal, UUID-looking extension values and exact Unicode/control-character
  legacy track identity survive. Raw UMP remains a muted, read-only Events view;
  the editor does not reinterpret its ordinals as audio frames or notes.

Captured UI states support local invalid/incomplete input rejection, dirty
Cancel/Discard and Save-before-track-continuation. The receipt records genuine
pointer/keyboard entry for the legacy placement, alongside accessibility-driven
controls. The source-window rejection and its explicit retry display the same
operation `c0524ec7-6409-42f9-8493-2865a0f1b56e`, expected revision 6, and verified
bounds 100–24100. That rejected operation is absent from the copied durable
ledger. Review latest followed by an explicit corrected new choice produces r7
without clamping or treating the saved 6100–18100 view as full source bounds.

The final saved JSON bytes equal the copied catalog's source r17 and copy r18
payloads. Each successful source r6–17 operation result also equals its saved
JSON bytes, with recomputed request/result hashes. Expanded original/source
panels and the scroll-to-actions screenshot show the retained original, current
settings and reachable Save/Restore controls. Actual unknown-outcome or concurrent
revision-conflict injection was not exercised here; those retain their separate
headless/service qualification.

## Preservation, complete archive and copy

The independent row audit recomputed every final copied-catalog row fingerprint
using the receipt's SQLite-value encoding: JSON arrays retain TEXT verbatim,
BLOB values use `blobHex`, compact UTF-8 serialization is hashed, and row hashes
are sorted per table. All **137 pre-existing rows across 12 projects** remain
exact. Only the synthetic source and restored copy add project identities. The
pre-restart and post-restart/output table sets, project IDs and immutable-take
lists match. SQLite integrity and foreign-key checks pass on both copied
catalog and archive.

All three pre-existing immutable take files and all 30 prototype project/asset
files were independently rehashed against their bound baselines. The synthetic
WAV, exact imported JSON and full owned take also match. The sealed staging
journal and owned journal have different physical-file hashes, as separately
recorded by ingestion; all seven journal tables are exactly equal, including
both original PCM/raw-event chunks, prefixes, descriptor and sealed manifest.
No source-byte rewrite is inferred from that physical snapshot distinction.

The 606,208-byte OSAR v2 archive contains one project, **14 history rows,
15 operation rows, one exact original import, one asset and one take**. Its
project, both history sides, every operation and import equal the source rows
in the copied catalog. Document and operation checksums recompute correctly.
The archive asset equals the original WAV. The take chunk sequence, lengths and
hashes validate, and concatenated archive bytes equal the entire 143,360-byte
owned journal, SHA-256
`c4a46085f862202de98388f925984e45906c20123752b345bf92464b0b683900`.

Copy r17 differs from source r17 **only at the root project ID**. The later
explicit rename changes only name and revision to copy r18. All 15 source-to-copy
operation mappings, their six hash/identity fields, mapping digest and immutable
proof digest were checked against actual copied ledger rows. Only typed request
project/operation IDs and root result project IDs change; opaque data and import
bytes remain exact. The first 13 copied history rows retain both documents with
only root IDs remapped. Renaming at cursor 13 legitimately replaces redo history
row 14 with r17→18; the mapped r16 Restore receipt and its original result remain
in the operation ledger. This is retained operation closure, not lost redo
evidence or a false claim that history sequence equals revision.

## Render, process and output boundary

The existing device-free render helper uses production `render::prepare` and
`RenderCursor`. Its note-specific receipt correctly leaves non-note changes
uninterpreted and does not itself claim that inputs were UI commits. The separate
whole-document/catalog checks above establish that connection for these files.
Independent RIFF/format, full-file and PCM hash checks plus exact four/eight-byte
sample/frame comparisons reproduce:

| Inputs | Original frames | Edited frames | Changed overlap frames | Changed overlap samples |
| --- | ---: | ---: | ---: | ---: |
| Saved r6 → saved r7 | 24,000 | 24,000 | 11,995 | 23,990 |
| Baseline r5 → saved r17 | 24,000 | 47,000 | 24,000 | 48,000 |

Both pairs are stereo 48 kHz float32. The final edit adds 23,000 frames beyond
the original render. These are offline rendered samples, not a measurement of
physical output correctness or latency.

The recorded process boundary reports initial PIDs 6498/6539, clean closure,
`pgrep` exit 1 with an empty bound output file, and a relaunch at PIDs 11642/11645
using the unchanged `216d…` executable. Captured post-restart UI states open
source r17 and copy r18 with retained current/original settings. This review
corroborates that historical receipt and the bound process text; it does not
claim an independent fresh process observation.

Recorded MacBook Pro Speakers output progresses through `playing` to
`playback_finished`, 48 kHz, two channels, 47,000 consumed prepared frames and
0 reported xruns, followed by explicit Stop. The screenshot itself states that
hardware latency and audible completion are unmeasured. Accept this as the
bounded native output/control observation. It does not establish listening
quality, acoustic completion, measured latency, elapsed recording or input
capture. No microphone/physical input or human performance was used.

## Final documentation boundary and remaining scope

The coordinator refreshed contract status and documented the already-reviewed
narrow ReleaseRejected metadata-retry context exception after freezing the
source review. Current documentation fingerprints are:

| Contract | Bytes | SHA-256 |
| --- | ---: | --- |
| `docs/contracts/NATIVE-CLIP-EDITOR-UI.md` | 8,424 | `53395b2e83afee57c5f091883cb200d8252b5efc4dd841269a845825f365c9db` |
| `docs/contracts/NATIVE-CLIP-EDITING.md` | 8,705 | `c4c6df5a1e211201f60e15053daf1b921ad9200f21a36ed16d1a0646fa9cce33` |

These documentation revisions do not change the 113 compiled inputs or the
package hash. The prior review's contract hashes remain dated snapshots.

Waveform/arrangement interaction, draft audition, full keyboard/IME/scaling
qualification, actual uncertain/conflict UI injection, performance capture,
metronome/count-in, monitoring/overdub/compensation, timing visibility, physical
input/listening/latency and elapsed-recording qualification remain open. MIDI 2
transport, diagnostic/project data, standard Clip Files and MPE remain distinct
requirements; model inference, plugin hosting, installers and Windows also
remain unqualified by this walkthrough.
