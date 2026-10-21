# Native source waveform implementation review

Reviewed on 2026-10-20 against the waveform work following baseline
`dbe88c72395dbd1452cbbc837c09acd3c0191513`. This is an independent source review
and isolated reader-test contribution. No Cargo, formatter, GUI, device, live
studio catalog, or original project was accessed by this reviewer. The earlier
`216d8e1c…` package qualification remains a separate historical boundary.

No blocking implementation finding remains in the reviewed reader, DTO,
preparation/client or UI paths. Source review accepts the bounded read-only
architecture; final integrated tests, exact package and actual waveform GUI
acceptance must be recorded separately. The source fingerprints below identify
this review snapshot, not a claim that a later build contains these inputs.

## Findings and disposition

1. **Failure versus retirement wording — corrected by the coordinator.** An
   ordinary bounded preparation error is a correlated completion and permits
   an explicit retry. A disconnected channel, invalid correlation or malformed
   successful result retires the read-only client. The contract now states this
   distinction. Neither class retires the durable session or audio worker.
2. **Growth after size preflight — corrected by the coordinator and independently
   tested in source.** Registered/physical size checks alone did not bound a
   file that grew during physical hashing. The current helper reads at most the
   remaining authenticated length plus one byte and rejects growth, using the
   minimum of caller ceiling and registered length. The added deterministic
   test appends 1 MiB at the first hash-read callback, after sealed verification
   and preflight, then requires the exact growth error and bounded block count.
   No reader success or source mutation by the reader is accepted.
3. **Fade constraints and DTO binding — design findings resolved in source.**
   Native fade lengths must have a checked sum no larger than the selected
   source-window frame count. Saved and retained-original native windows both
   obey that rule. Legacy seconds retain their existing 1e-9 tolerance;
   independently rounded endpoints/fades can produce one frame of overlap.
   The UI handles that compatibility case while rejecting overlapping native
   proposals. The DTO now binds identity, format, exact coverage, settings and
   the content/summary budgets rather than validating peaks alone.

## Reader and source-authentication assessment

`NativeReader::open` opens READ_ONLY/NO_MUTEX with query_only, a bounded SQLite
cache and mmap disabled. It requires a real root and ordinary nonsymlink catalog,
rejects hardlinked catalogs on Unix, checks the rollback-journal header before
SQLite open, and accepts only current schema 4. It exposes no mutation API and
never creates a root/catalog or migrates a schema. These checks concern the
trusted local studio namespace. They are not a hostile-filesystem sandbox or
removable-volume identity proof.

`read_snapshot` owns a deferred transaction, explicitly rolls back on both
success and returned error, and relies on transaction RAII for panic rollback.
Preparation resolves the project ID/revision, exact clip address, source key,
selected metadata and selected legacy bytes inside that short snapshot. The
snapshot ends before native source scanning. Metadata-only loading therefore
does not open unrelated take objects. A valid selected asset remains readable
when an unrelated asset or take is damaged, while the damaged source's own
lookup fails.

The selected asset path validates the content-addressed ID, checks SQL byte
length before allocating a BLOB, and verifies its checksum. Native preparation
preflights the complete registered object size and complete-manifest chunk
count, including other streams. The bounded source opener checks exact
reference/manifest/stream, ordinary file identity, registered and physical
length, full sealed identity, complete physical hash and final file identity.
The post-snapshot opened manifest and object filename must match the earlier
metadata snapshot. Existing publication/save authentication continues through
the same helper with the old caller ceiling and retains full registered-length
verification.

The initial sealed scan and an individual OS read are not interruptible. A
small regular file can still stall on external storage. Cancellation checks
surround the scan, occur before each 64 KiB physical hash block and are checked
during subsequent decoding. Hide revokes display; it does not claim I/O has
stopped. The API does not establish an I/O deadline, driver qualification or
protection against concurrent hostile ancestor replacement.

## Numerical and lifecycle assessment

The backend reads actual RIFF/WAVE PCM16/24/32 or float32 and native
S16/S24/F32 planar/interleaved PCM. Unsupported codecs, malformed extents,
non-finite PCM and channel/rate mismatch fail explicitly. Shared decoder
parameterization leaves public playback wrappers at eight channels while
waveform decoding admits up to 32 WAV or 16 native channels. It never invokes
arrangement preparation, applies gain/fades, folds channels, clips samples or
normalizes the source data.

Native source origin and sealed-prefix end remain checked integer frames;
arrangement placement remains a separate signed frame or legacy tick value.
Exact u128 partitions cover the source domain, including the final sample.
Min/max begin with actual content, avoiding a zero bias on positive-only or
negative-only signals. Gap intervals advance by bin boundaries rather than
missing-frame iteration. Mixed bins preserve content extrema and gap counts;
gap-only bins have no extrema. Recovered unknown tails have an explicit flag,
without fabricated duration beyond the proved prefix.

The limits are 30 MiB for a legacy asset, 64 MiB for a complete native object,
65,536 whole-object chunks, 32 × 1024 × 1024 selected decoded channel samples,
1–1024 bins per channel and a 2 MiB typed summary including actual allocated
bin/Vec capacity. There is no waveform duration cap. Native content-count
preflight precedes authentication/decoding and actual decoded work is checked
again. These are configuration bounds, not execution-time measurements or a
30-minute waveform qualification.

`WaveformClient` retains one admission until a correlated completion is
consumed. Cancel changes its generation without freeing credit; cancellation
also suppresses an already queued success. Stale/unmatched results cannot
silently replace the active selection. Correlated malformed data retires the
worker; an uncorrelated result retires it while retaining credit until channel
disconnection. Drop revokes publication and drops channels without joining a
blocked worker. There is no timeout path that launches another worker over
unfinished I/O.

## Independent UI assessment

`waveform_ui.rs` builds a small key from project ID/revision, exact clip address,
selected immutable source/rate and resolution. It borrows the document/clip;
paint never clones the document or parses full recording metadata. Source,
revision, selection or copied-project changes revoke the active result and
clear the displayed summary. Completion installation independently checks the
ticket, complete request, current selection and returned data request.

Main polls waveform progress before ordinary busy/loading rendering and
schedules a repaint while it is pending. The waveform pane sits outside the
disabled session-edit scope, so Hide/Cancel remain available while a durable
operation is busy. New Show/Retry requires an available studio and unused
waveform admission; navigation does not enqueue a replacement automatically.
The client owns no session edit/retry/continuation state.

The source graph stays visible when valid legacy declared metadata extends
beyond the real WAV and only its saved/original overlay is unavailable. A valid
local repair can still preview against authenticated bounds. Invalid,
unresolved, stale or incomplete drafts produce an explanation and retain valid
saved overlays. Preview applies only a bounded typed Keep/Set/Remove patch to
cached settings, preserves exact editable Number text and never submits an edit,
creates editSource, changes schema, alters loop extent or auditions audio.
Native saved/current-original out-of-prefix windows remain authentication errors;
they are not quietly accepted as useful overlays.

Channels use bounded virtualized rows. Raw amplitudes determine an explicit
view scale, without mutating peaks. Source-origin subtraction happens in integer
space before screen conversion, preserving short windows above 2^53. Saved,
permanent-original and local draft outlines have separate labels and solid,
dotted and dashed styles. Missing coverage is hatched and explained; no guessed
sub-bin gap location or unknown-tail extent is drawn. The gain outline uses the
renderer endpoint/product-then-sine law and is labelled a visual projection.

## Independent tests and current evidence boundary

`crates/omniusong-store/src/reader_tests.rs` contains 12 isolated synthetic tests:
missing/empty catalog; old/future schema; WAL header; root/catalog aliases;
exact document and selected-only reads; asset size/hash rejection; snapshot
release on success/error; panic rollback; registered/physical budget-before-
corrupt-scan; cancellation before open/during hashing; post-preflight growth;
and physical hash corruption despite valid sealed logical rows. Fixtures use
fresh temporary directories, compare byte fingerprints/sidecar inventories and
retain failed fixtures for diagnosis. No live catalog or device is involved.

The coordinator's initial `check-initial.log` passed production compilation and
`engine-initial.log` records 18 passed waveform/backend/DTO tests. Those logs
precede the last two backend client tests and the independent reader tests.
The initial workspace attempt in `workspace-initial.log` caught a misplaced
independent-test block referencing an out-of-scope variable. That fixture error
was corrected and the 12-test source frozen; it does not count as a passed gate.
No fresh integrated test, formatter, Clippy, package, offline qualifier or native
waveform GUI acceptance is claimed in this snapshot. Evidence paths are under
`.runtime/rust-waveform-ui/`.

## Reviewed source fingerprints

Captured after the source review; later mechanical formatting requires a fresh
fingerprint addendum before exact build evidence is attributed to these files.

| Path | Bytes | SHA-256 |
| --- | ---: | --- |
| `crates/omniusong-store/src/reader.rs` | 5195 | `054d3ffc02dfbdbbd166ace59c0f80346b6417c06a39cf15c1387ba5b5ee0ce6` |
| `crates/omniusong-store/src/reader_tests.rs` | 22818 | `28b90e36a63e8d9d4e23e3f6b6b45c4a8eb6c08dea80e0a34390a64ad19c64f5` |
| `crates/omniusong-store/src/takes.rs` | 30229 | `0f27783f6156fbdfe1767f12b83e1ef620fb0e6dd5e465ddb30e2b2cd344c412` |
| `crates/omniusong-engine/src/waveform_types.rs` | 19483 | `1b683f30256ef2ed8f9284b5dfbb2a201b7bf390fb6d642fd293c2203cbefc3c` |
| `crates/omniusong-engine/src/waveform.rs` | 52624 | `ceade19714b5131a2daf69dc613ee56803abcb4d9f83d2a12c01dae6577ee2ac` |
| `crates/omniusong-engine/src/render.rs` | 21641 | `cbae2f4b3244ce70f34400984549c9fd55c8836d12ee14832e0294b5f0e3d623` |
| `crates/omniusong-app/src/waveform_ui.rs` | 69687 | `c1205a744d96e645d02be586e1822cf9c287f0f439faa6626b233b89cc17e70e` |
| `crates/omniusong-app/src/main.rs` | 158241 | `c2cc2ce8070de9902685ccccd971dbb66fc334299a4cda1f97c783a03a8a2e2e` |
| `crates/omniusong-app/src/clip_ui.rs` | 64451 | `c9736c0c28756068aeb1d0be3feb87459bda4c7f4408d8de6f9c23df1e9f7010` |
| `docs/contracts/NATIVE-SOURCE-WAVEFORM.md` | 6427 | `50e0491c6849396932eb4ecbebef79faf2b17a6caf86068d7b24b1c7bfa0122a` |
