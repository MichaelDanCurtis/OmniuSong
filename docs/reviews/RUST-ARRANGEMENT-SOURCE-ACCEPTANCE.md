# Native arrangement source acceptance

Reviewed October 20, 2026 local time (October 21 UTC artifacts).

**Accept the bounded source/headless/build package. Native arrangement workflow
acceptance remains pending.** No remaining source blocker was found. The exact
`09789029…` package has startup evidence only; earlier waveform/clip walkthroughs
remain tied to their own binaries. Milestone 2 and the full goal remain open.

## Independent checks

The [source receipt](../verification/rust-native-arrangement-tests.json) and
`.runtime/rust-arrangement-ui/gate1-run.json` have identical command records and
source arrays. All **108 distinct source snapshots** and **117 bound fingerprint
records**, including binary/plist/logs/frozen architecture, match recomputed
SHA-256 and byte lengths. Every Rust file and crate manifest under `crates/` is
included, plus root manifests/lock/config, the bundle script and arrangement
contract. This is the receipt's source set, not fresh dependency qualification.

Independent parsing of all 45 workspace result groups gives **562 passed, zero
failed, two ignored**, including **173 app tests**. All ten math-owner, sixteen
UI-owner and nineteen independent arrangement cases pass. The two ignored
entries are existing private subprocess helpers, not device tests. Coordinator
records/logs show exit 0 for formatting, offline/locked workspace tests with all
features, warnings-denied all-target/all-feature Clippy, all-target workspace
build, Mac packaging and strict deep signature verification. I did not rerun them.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Source receipt | 24,491 | `eb97e9dca3648c5e62267867df0def52d337511c382f03411972839d75a99604` |
| `gate1-run.json` | 22,503 | `228ad435d82205df1aa1deee8b93240b7480bc90cda49f284d1a03462c0887b5` |
| `gate1-workspace.log` | 62,247 | `450cf78e1ccdb9e2a7b606511ac82d6c6c8f4714b919d44de463ee8c0c83fe88` |
| Packaged `OmniuSongNative` | 88,334,128 | `09789029f54d2d3c9fff0b94d22a3176afe483bc6b439a0cf703d151338f84ec` |
| Frozen architecture review | 31,540 | `c8b3b677ab1c01345bbb3f288bccb0a36de4601c1e2336865cadc1c35cf3b4db` |
| Independent integration tests | 33,062 | `dbc4b90e1b2f545d44a1e0cc9e36d4783ede0b48221bd89c73e4505eea8d22d5` |

## Final source disposition

Boxing `ClipContext` in UI/main actions retains the complete captured payload;
main forwards it unchanged and the editor borrows it for the same identity,
serial, raw-draft and source checks. This is ordinary UI allocation, with no
wire, durable-operation or audio-callback change. The final integrated tests
exercise that representation.

The consequential fixes are present and passing: rounded sub-unit movement is
inert before snap; every document install invalidates cached geometry/drag even
at unchanged ID/revision/tempo; saved/original/local views have independent
visibility with only effective-current geometry draggable; clipped-out row space
cancels release; and normal egui drag recognition rejects one-pixel click jitter.
Placement still uses the existing guarded clip draft and explicit Save/Discard.
Exact native frames/source windows, original metadata and Number spelling survive
local geometry. Legacy drawing's exact-start-plus-binary-duration estimate remains
distinct from core's authoritative floating loop-extent calculation.

The valid 32,000-note fixture replaces an invalid 80,000-note pretty-JSON fixture;
the document ceiling was not weakened. Cache construction scans bounded metadata,
paint limits individual sources to 256 with a selected-source reserve, and exact
selectors remain available. These are software bounds, not native frame-rate or
source-authentication measurements. The package is ARM64/ad-hoc-signed development
output, not a notarized installer or Windows qualification.

## Preservation and runtime limit

The [preservation receipt](../verification/rust-native-arrangement-source-preservation.json)
is 2,261 bytes, SHA-256
`ea8b0d4e2cd0f642cb43662283abf59f8f6e40c13bb2b6a8bd8814d269f764ce`.
Its bound before/source-gate/initial-launch JSON snapshots differ only in timestamps:
**14 projects, 220 row hashes across ten tables, four immutable-take fingerprints**,
integrity result and whole-catalog fingerprint remain identical. The recorded
catalog is 7,757,824 bytes, SHA-256
`994f9cdd6b591363211972d22b47dbe82c3246c47da8317fc8068abeddcceff0`.
This review compared the snapshots without opening the live catalog/take objects.
Separately, all **30 prototype files** (19 projects/11 audio files) match direct
hash/length checks against `rust-migration-baseline.json` (7,170 bytes,
SHA-256 `1169fdd5337f98798b0dbd6fb2b87c69ff5f8ab6759ad7454f4c587e3b25536e`).

The [runtime receipt](../verification/rust-native-arrangement-ui.json) explicitly
says **`completed:false`**: 2,155 bytes, SHA-256
`43dbb54f9fc07802b776268ac90b5d762f3548c0d721ccc7e4ab57b0f69840e8`.
Its retained `.runtime/rust-arrangement-ui/initial-runtime-pending.json` copy is
byte-identical; source/binary/AX/screenshot/stack/preservation links match. The
AX and screenshot show loading, not arrangement controls. The sample shows the
session thread waiting through SQLite `posixOpen`/`__open` while UI execution
continues. A drive-permission prompt as the cause remains an inference from
earlier launches; these artifacts do not show or answer that prompt.

Current README/STATUS/plans/release matrix preserve this pending boundary. The
coordinator corrected two stale sentences to separate the already-qualified
read-only waveform from future waveform manipulation; source/package stayed
unchanged. Actual arrangement selection/drag/nudge, Save/Discard/history,
archive/copy, post-edit preservation, restart/reopen and output on `09789029…`
still need their own receipt. No input, listening, latency, long recording,
MIDI/MPE, model/plugin or full-workstation claim follows. This review ran no
Cargo, formatter, native GUI/device or catalog operation and edited only this file.
