# Native waveform gate-1 evidence audit

Independent audit on 2026-10-20 (local date). **Gate-1 source/headless and the
bounded offline waveform evidence are supported. Initial native waveform UI
acceptance failed because the bottom of the graph was clipped.** No later gate,
replacement package or corrected GUI is accepted by this review.

This reviewer ran only bounded file hashing, read-only SQLite queries against
an isolated retained catalog and retained synthetic source, and independent
Python PCM/bin calculations. No Cargo, formatter, native GUI, device or live
catalog SQL was used. Existing evidence and source files were not modified.
The four immutable owned take files were rehashed read-only. Only this review
was written.

## Gate and source binding

All six command logs named by `gate1-run.json` exist and match their recorded
SHA-256 and byte length. Recorded exit codes are zero for formatting check,
offline workspace/all-feature tests, offline all-target/all-feature Clippy with
warnings denied, offline all-target build, Mac packaging and strict deep
signature verification. Independently summing the workspace log's 45 result
groups gives **516 passed, 0 failed, 2 ignored**. These are the gate-1 results;
this reviewer did not rerun the commands.

The gate receipt retains **120 source fingerprints**. At audit, 119 still
matched current files exactly. The sole difference was the authorized layout
correction in `crates/omniusong-app/src/waveform_ui.rs`. Its gate-1 fingerprint
is 69,687 bytes, SHA-256
`c1205a744d96e645d02be586e1822cf9c287f0f439faa6626b233b89cc17e70e`.
The gate runner captures source fingerprints before executing the commands and
checks the complete set again before writing its passed receipt. Gate-1 results
must not be transferred to the later layout source without a fresh gate.

The initial exit receipt identifies binary SHA-256
`1b70766857d573cdb0b9067334864d4ecfb80e47c23e9969c11cea5e23431d17`.
During this audit the mutable packaged executable had already been replaced by
the coordinator's next build. Therefore the old binary identity is supported
by the historical receipt and recorded run, **not a new byte-for-byte rehash of
an archived gate-1 executable by this reviewer**. The new executable and any
subsequent signature/build/UI evidence are outside this initial audit.

## Offline source and bin evidence

`offline-waveforms.json` is 673,101 bytes and exactly matches the fingerprint
bound by `independent-bin-audit.json`. The bound `audit_waveforms.py` also
matches. Its inspected code uses read-only SQLite connections and independent
RIFF/float32 parsing, compares integer bin intervals and packs extrema back to
float32 for equality. It does not invoke the Rust decoder, device APIs or GUI.
The script writes only its new audit receipt. This reviewer did not rerun that
script against the live studio.

Instead, the independent recomputation used:

- The already retained `.runtime/rust-clip-ui/catalog-after-clip-ui.sqlite3`,
  opened with `mode=ro`, one read transaction, successful integrity check and
  no foreign-key violations.
- The retained synthetic WAV and `synthetic-original.sqlite3` named by the
  original clip fixture. Their hashes match the fixture. Native chunk PCM is
  the same authenticated synthetic content as the registered owned snapshot;
  the source SQLite file and its owned snapshot have distinct physical hashes,
  which are not conflated.
- The source project's exact stored JSON: project
  `811b10bd-b767-41db-997e-bd9b449aabee`, revision 17, 5,987 bytes, SHA-256
  `9519d7144c3c954a0f438f1b1480d2b67b948011a41b34e40914c53025fca15f`.
  Both cached saved/original settings were compared to this stored document.

All **3,072 bins** independently match exact floor-divided integer intervals,
content/gap counts, and float32 min/max bits from the retained PCM. Each view
has one channel, 48 kHz, 24,000 original frames and 1,024 bins. The native views
share one underlying stream; this is three selected views, not three distinct
recordings.

| Selected view | Complete source domain | Saved source window / placement | Permanent original window / placement |
| --- | --- | --- | --- |
| Legacy audio | `[0, 24000)` | `[3000, 21000)` / 321 ticks | `[6000, 18000)` / 123 ticks |
| `native-audio-first` | `[100, 24100)` | `[1100, 10100)` / −480 frames | `[6100, 18100)` / 0 frames |
| `native-audio-second` | `[100, 24100)` | `[100, 11100)` / 36,000 frames | `[100, 12100)` / 12,000 frames |

Legacy windows above are the labelled renderer-rounded projections of stored
seconds; exact Number spelling is preserved by the unchanged document and table
hashes, not by the f64 projection. Current legacy fades project to 480/0 frames;
original fades project to 600/1,200. Native first current fades are 480/960 equal
power with original null; native second current fades are explicit zero/zero
linear with original 100/200 equal power. Raw source extrema remain unchanged
by these settings, positions or fades. The shared extrema are approximately
−0.0393964946 and +0.0395016670.

The raw-events clip is explicitly skipped with `interpretedAsAudio: false`,
source units `events`, count 4, and no fabricated audio duration. All three
qualified source views in this fixture have zero declared gaps and no unknown
tail. This particular offline receipt therefore does **not** independently
qualify recovered gaps, unknown tails, wide channels, long recordings,
unsupported codecs, listening, playback/recording, realtime coexistence or
latency. Those broader synthetic tests and future device qualification remain
separate.

## Preservation evidence

`before-waveform-store.json` and `after-initial-ui-store.json` are exactly equal
apart from `recordedAtUtc`. Both record integrity success, the same 14 project
IDs, the same schema/column lists and every row hash across all 10 tables:

| Table | Rows |
| --- | ---: |
| archive_copies | 5 |
| archive_copy_proof_operations | 38 |
| archive_copy_proofs | 5 |
| archive_project_lineage | 6 |
| assets | 1 |
| edits | 62 |
| imports | 3 |
| native_takes | 4 |
| operations | 82 |
| projects | 14 |
| **Total** | **220** |

This reviewer recomputed all 220 row hashes from the isolated retained catalog,
using the recorded columns, exact SQLite string values and tagged blob hex,
then compared each table's sorted hash list. All match both receipts. Stored
project/import/request/result JSON remains a string during this hashing, so
opaque numeric spelling and null/absence are not reserialized through Python's
JSON-number parser. The isolated backup's physical SHA differs from the live
catalog's recorded physical SHA; row equality is checked explicitly rather than
assuming backup files are physically identical.

Both preservation receipts record the same whole catalog file: 7,757,824 bytes,
SHA-256 `994f9cdd6b591363211972d22b47dbe82c3246c47da8317fc8068abeddcceff0`.
This is a comparison of the captured physical fingerprints; the reviewer did
not reopen/query the live catalog. The four immutable take fingerprints also
match between receipts, and independent read-only rehashing confirmed their
current bytes and lengths:

| Owned object SHA-256 | Bytes |
| --- | ---: |
| `679914498bf02ced1570f8b58ba441eb1f958e8dec54d2931a4eab9f3657ff6b` | 40,960 |
| `8460ff93703afb9c89ac7359e45761e9605fb1a1ae029aa8dc068398e11bd897` | 917,504 |
| `e654b488b83a63d9323615e5b725dafbb8565ac901621c12954c1885ee428430` | 139,264 |
| `c4a46085f862202de98388f925984e45906c20123752b345bf92464b0b683900` | 143,360 |

This supports the bounded no-catalog/source-change claim for the offline and
initial Show/Hide walkthrough interval. It does not claim drive-disconnect,
power-loss or adversarial concurrent-writer resilience.

## Initial GUI and process disposition

All eight named initial steps have both AX text and PNG artifacts, with unique
filenames. AX step 04 and later displayed the correct selected project/revision,
asset hash, source `[0, 24000)`, saved `[3000, 21000)` and permanent original
`[6000, 18000)` labels. Step 08 returns to **Show source waveform**, supporting
the observed Hide transition. Repeated identical AX hashes for steps 04–07 are
consistent with scrolling a graph whose semantic text did not change.

Visual inspection of `07-legacy-source-full-lane.png` confirms the lower graph
is clipped by the central/bottom layout. Its filename is descriptive capture
history, not proof that a full lane was visible. The initial process receipt
correctly states **not qualified** and records a clean close followed by pgrep
exit code 1 with no matching processes. This reviewer did not run a fresh
process query or operate the GUI.

The evidence supports correct initial data binding and Hide, while failing the
full-lane visibility criterion. Corrected layout source, a fresh gate/package,
and actual corrected waveform walkthrough remain required. No native input,
listening or latency acceptance is inferred from these source-overview actions.

## Audited artifact fingerprints

These hashes bind the exact initial evidence audited here. Existing source
reviews and their historical fingerprints remain unchanged.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `.runtime/rust-waveform-ui/gate1-run.json` | 24662 | `056748d96dd5878be2c3db4e6641a28081c7d1a991a2273f96681b49f513f4cb` |
| `.runtime/rust-waveform-ui/offline-waveforms.json` | 673101 | `5456cf22948e053fcf0a04b4efe7fa575a86d35fbf5d93c1fd1d96398aab6622` |
| `.runtime/rust-waveform-ui/independent-bin-audit.json` | 2792 | `7e1b9eafe227ae5bc8246ad71a59f19e93bcb8f5111edfb7e9abe8f72738f454` |
| `.runtime/rust-waveform-ui/audit_waveforms.py` | 6059 | `292ebe7a80cf9390022e953061a98f5595cbc77305c4915b0811ed75f78f9cab` |
| `.runtime/rust-waveform-ui/before-waveform-store.json` | 20587 | `2b693197fce3888ad75becd850e90a8366c5d61611208b2c4d6e72a290f42134` |
| `.runtime/rust-waveform-ui/after-initial-ui-store.json` | 20587 | `c416d5259c4568dad4508f7735ceeefe3ba6dcce2094d005eef9fb83ee18d2a3` |
| `.runtime/rust-waveform-ui/initial-process-exit.json` | 347 | `afad5069cb03cd22461654d9f9f2c8d361e97a2ccee78b43e6e128dbb2817acc` |
| `.runtime/rust-waveform-ui/ui-steps.json` | 774 | `9792facc66fd3a007f40cde12009b7289a46c04045c7e6680498d62b4f3ab926` |
| `.runtime/rust-clip-ui/catalog-after-clip-ui.sqlite3` | 7757824 | `f18cfbb07e71d0ec1922bc949e65d48842a0ef221b2566f3b5bd80856f6a363f` |
| `.runtime/rust-waveform-ui/gate1-fmt.log` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `.runtime/rust-waveform-ui/gate1-workspace.log` | 57551 | `3d4f7d74cb89767a95f80f0d17c84b8f60aaad575275d1e81ea1c4386f07b4ba` |
| `.runtime/rust-waveform-ui/gate1-clippy.log` | 498 | `1a2d0433bb583748bc906384d2915b348cd4ec979a19fd37f245ece207e2f2cc` |
| `.runtime/rust-waveform-ui/gate1-build.log` | 498 | `795760a2777862e76259ebe01195ac876f2d2b4c95645ade6170d2837e4a51c5` |
| `.runtime/rust-waveform-ui/gate1-package.log` | 589 | `6209f9de6eca12295cbe91a279caa64030d0c3cbddba4d3c9a46e8455362ffaa` |
| `.runtime/rust-waveform-ui/gate1-signature.log` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `.runtime/rust-waveform-ui/01-launch-loading-or-ready.png` | 144080 | `e8dad92e46c6e37454f0700087d58b36b1c19543779fa0e5e2bef14454a783db` |
| `.runtime/rust-waveform-ui/01-launch-loading-or-ready.txt` | 4971 | `a872159e22cd6fb7534094df6a572730747c44a9d5aeff5d1b9efc45ba7dc38e` |
| `.runtime/rust-waveform-ui/02-source-r17-opened.png` | 176604 | `ccb5d55795428708cea7c7496de31675d01980f4aa47cc4631d83d0719516b74` |
| `.runtime/rust-waveform-ui/02-source-r17-opened.txt` | 5921 | `b9a5dfc0a752c069dc67345d965f8cee6abab0707d6ae02153559cbea55b6f4b` |
| `.runtime/rust-waveform-ui/03-legacy-selected-before-show.png` | 216325 | `959f6a330d96da361d97726e34624b44205cff4118ca153f8c985a91cbb08a99` |
| `.runtime/rust-waveform-ui/03-legacy-selected-before-show.txt` | 7830 | `6e69a7267724f03d5bfa8ef5a7043279254cb0779d250820ad44be756aedb079` |
| `.runtime/rust-waveform-ui/04-legacy-authenticated-source-and-original.png` | 252937 | `8f3bc01da906276979f09a795fc58d7b6cb01fb4e3a7427e2bdbc585ab54c6bf` |
| `.runtime/rust-waveform-ui/04-legacy-authenticated-source-and-original.txt` | 8914 | `f0f529b70c968aa4c5b49f4792a093dfa68e29c1975fdcd06eb648fa270f1281` |
| `.runtime/rust-waveform-ui/05-legacy-graph-visible.png` | 252945 | `09720baccace60a63b0ce3b0da98b6963de9c3e50ca33d8d19c7eb2b7d5b7781` |
| `.runtime/rust-waveform-ui/05-legacy-graph-visible.txt` | 8914 | `f0f529b70c968aa4c5b49f4792a093dfa68e29c1975fdcd06eb648fa270f1281` |
| `.runtime/rust-waveform-ui/06-legacy-source-graph-scrolled.png` | 254371 | `dba5f975cfd76bf4df5bd880d985cf668f6ae6cb54d206c4b950b33670473008` |
| `.runtime/rust-waveform-ui/06-legacy-source-graph-scrolled.txt` | 8914 | `f0f529b70c968aa4c5b49f4792a093dfa68e29c1975fdcd06eb648fa270f1281` |
| `.runtime/rust-waveform-ui/07-legacy-source-full-lane.png` | 254778 | `29c7dd84f54a762b2298f49ff4e493a671dfb9c8b19c64adfd97f0fc39e0f33c` |
| `.runtime/rust-waveform-ui/07-legacy-source-full-lane.txt` | 8914 | `f0f529b70c968aa4c5b49f4792a093dfa68e29c1975fdcd06eb648fa270f1281` |
| `.runtime/rust-waveform-ui/08-initial-build-hide-before-layout-fix.png` | 215859 | `8adf7b4bef2f4615470113627e584cb9d2366b5ad9d7d7a6b397260e27f1621b` |
| `.runtime/rust-waveform-ui/08-initial-build-hide-before-layout-fix.txt` | 7830 | `6e69a7267724f03d5bfa8ef5a7043279254cb0779d250820ad44be756aedb079` |
