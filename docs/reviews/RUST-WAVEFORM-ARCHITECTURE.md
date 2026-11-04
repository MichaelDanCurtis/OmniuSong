# Native source waveform architecture review

Reviewed against `dbe88c72395dbd1452cbbc837c09acd3c0191513` on 2026-10-20.
This is an implementation recommendation, not waveform implementation or UI
acceptance. The qualified `216d8e1c…` clip-inspector package remains the baseline.
No build, GUI, device, live catalog or source mutation was performed for this
review. Only this document was written.

The dated design corrections below were added on 2026-10-20 before the new
source gate. Subsequent implementation review and independent reader tests are
recorded separately in `RUST-WAVEFORM-SOURCE-REVIEW.md`; they do not transfer
the earlier package's GUI acceptance to this new waveform work.

## Adopted direction

Use a dedicated, bounded `WaveformClient` in the engine crate, with a worker-owned
read-only source resolver. Do not route preparation through the durable session
service or the audio-worker process. Reuse the supported PCM decoder primitives;
do not render the arrangement to manufacture a source waveform. Return real,
per-channel source minima/maxima with explicit gap coverage. Draw saved,
permanent-original and valid local-draft windows as separate UI overlays.

The coordinator accepted this direction and refined the initial limits below.
The newly authored `engine/src/waveform_types.rs` defines `WaveformRequest`,
`WaveformSourceKey`, `WaveformSettings`, `WaveformBin` and `WaveformData`. Client
member names below remain recommendations for the shared contract freeze; the
presence of types does not establish preparation, lifecycle or UI implementation.

The initial session-command proposal would reuse existing correlation and
storage ownership, but `SessionService` has one request/reply credit and a single
worker (`crates/omniusong-session/src/lib.rs:470`). Its busy state deliberately
guards durable operations and navigation. Giving a source overview that same
credit would disable unrelated navigation/Hide or require changing mature
completion and retry paths. A separate read-only worker lets editing/navigation
continue and lets the UI revoke a stale display without changing durable intent
semantics. It needs its own retained admission credit; merely moving work to a
thread does not bound outstanding jobs.

## Existing-source hazards to resolve before implementation acceptance

1. **Existing-only is not read-only.** `NativeStore::open_existing` calls the
   READ_WRITE path and can migrate an older catalog
   (`crates/omniusong-store/src/lib.rs:321`). The proposed new `NativeReader`
   must open READ_ONLY/NO_MUTEX with `query_only`, accept only the current schema,
   create no directory/catalog and perform no migration. The coordinator plans
   to reject a WAL-format header before SQLite open. Keep the current trusted
   local-filesystem boundary explicit: pre-open header checks do not authenticate
   arbitrary concurrent same-user path replacement or solve removable-volume
   identity.
2. **Ordinary document load does too much for a selected overview.**
   `NativeStore::load` verifies every referenced take in the project
   (`lib.rs:488`); an unrelated huge/damaged source could block this selection.
   The read-only capability should validate the bounded document and exact
   selected source, without claiming whole-project media integrity. Read the
   document/revision and selected registry metadata in one short catalog
   snapshot, then release that transaction before scanning immutable media.
   Holding a main-catalog read transaction for the entire scan would obstruct
   unrelated saves under the existing rollback-journal model.
3. **Authentication precedes sampling and needs its own bound.**
   `open_take` authenticates the complete physical object; `hash_file` currently
   loops over 64 KiB reads without cancellation (`takes.rs:249–288`). The new
   bounded resolver must check registered and physical lengths before expensive
   verification, preserve existing identity/hash/file protections, and check
   cancellation during physical hashing. `SealedJournal::open` performs sealed
   verification before returning (`snapshot.rs:43–70`); the coordinator will keep
   that existing verification unchanged. Therefore its bounded verification and
   individual OS reads remain explicitly noninterruptible. Cancellation checks
   before/after them do not create a latency guarantee.
4. **Arrangement preparation is the wrong source.** `render::prepare` applies
   mute/gain, clips/fades, output-channel/rate rules and procedural notes
   (`render.rs:244` onward). Reuse `inspect_wav`, `wav_sample` and `pcm_sample`,
   which validate actual supported PCM and reject non-finite values. Never call
   `prepare` or apply the output limiter to waveform data.
5. **A preview must not clone the project in paint.**
   `ProjectDocument::update_clip` clones the document (`clip_edit.rs:237`). Use
   a small known-settings projection/validator for local overlays. Do not call
   whole-document mutation, parse an entire recording payload, or serialize
   large opaque metadata on each frame.

## Minimal request, response and lifecycle contract

The following shapes provide the needed ownership without expanding the session
service's commands or creating a durable operation:

```rust,ignore
WaveformRequest {
    project_id: String, // validated as ProjectId
    expected_revision: u64,
    address: DocumentClipAddress,
    source: WaveformSourceKey,
    bins: usize, // checked 1..=MAX_WAVEFORM_BINS
}
WaveformTicket { generation: u64, sequence: u64 }
WaveformCompletion { ticket, request, result: Result<WaveformData, WaveformError> }

WaveformSourceKey::LegacyWav { asset_id, source_asset_id }
WaveformSourceKey::NativeAudio { source: TakeReference, sample_rate: u32 }

WaveformClient::spawn(root: PathBuf) -> Result<Self, SpawnError>
client.try_submit(request) -> Result<WaveformTicket, SubmitError>
client.cancel(ticket) -> bool
client.try_poll() -> Result<Option<WaveformCompletion>, TransportError>
client.is_busy() -> bool
```

`spawn` only starts a worker; opening/validating the reader happens there. A
rejected submission returns its request. Request IDs and local display epochs
must advance without wrap; they are not project revisions or operation IDs.
The exact project, revision, address and expected source must match the saved
document before preparing. Never fall back to a track/vector index, neighbouring
clip or current project when a requested identity disappears. Native extraction
requires schema 2, recording kind and AudioFrames, not opaque similarly named
fields. Locks/mutes do not prevent read-only review. Events and notes produce
an explicit unsupported-source result rather than silent or synthesized peaks.

The worker returns the echoed request plus authenticated source identity:
legacy asset SHA/byte length; or take/manifest/object SHA, stream and verified
descriptor/prefix. For legacy audio, plot **assetId**, the selected playable
source. `sourceAssetId` is retained identity/provenance and can identify ancestry;
never substitute its bytes silently or label the plotted asset a human original
without provenance supporting that statement.

Keep one persistent worker and capacity-one request and completion channels.
Retain admission until the actual matching terminal completion has been
consumed, including cancelled work. Hide, selection changes and navigation
revoke publication and set cooperative cancellation, but do not free the credit
or spawn another worker. New Show while the old job unwinds reports busy. An
explicit retry is a fresh read request after completion, never a durable edit
retry and never a replacement for the main app's retained mutation intent.

The UI polls even when the plot is hidden or another session is selected, so
obsolete completions release credit. Before installing a result it checks ticket,
request identity, current project/revision/address/source and local visible-view
epoch. Draft text changes alone need not invalidate raw source data: draw the
latest validated local overlay. Saving/Undo/Redo, source replacement, reconnect,
selection away-and-back and Hide invalidate the display epoch. A success arriving
after cancellation must remain discarded. Initial implementation needs only one
displayed summary, not a multi-source cache. Any later cache must key by verified
content identity, stream and resolution, and distinguish cached data from fresh
source authentication.

Drop sets the worker's alive/cancel flag and closes channels without joining,
waiting or file access on the UI thread. A stalled OS read can outlive the window;
do not claim it stopped. Preserve one terminal transport cause rather than
repeatedly replacing it with EOF. Keep typed reader error provenance until policy
classifies `Cancelled`, `StaleSelection`, `Unsupported`, `BudgetExceeded`, damaged
source, I/O/database failure or protocol failure. All are read-only outcomes;
none has an uncertain project commit. A fault in this separate reader does not
silently retire or replace the session service's connection.

## Source domain, PCM and bounded bins

The summary should contain `source_start_frame`, `source_end_frame` (exclusive),
`sample_rate`, channel count, encoding/layout description, per-bin coverage and
per-channel `Option<Peak { min: f32, max: f32 }>`. Include explicit unknown-tail
state and recorded/gap totals. IDs/descriptions retain existing bounded typed
validation; do not return arbitrary source metadata or PCM buffers to the UI.

For WAV the source domain is `[0, decoded_frames)`, determined by RIFF/data bytes,
not declared duration. For native audio it is
`[descriptor.initial_source_position, sealed_prefix.source_end.value())`.
This can start at 100 or another nonzero ordinal. Current/original clip ranges
do not define full source bounds. Source units stay unsigned frames; arrangement
placement remains signed frame/rate or legacy startTick metadata and never shifts
the source axis. Display seconds as a derived source quantity with the frame
origin stated; use checked integer subtraction before floating-point drawing.

For `n = min(requested_bins, source_frame_count)`, bin `i` covers
`start + floor(i * count / n)` through
`start + floor((i + 1) * count / n)`, exclusive. Use checked widened arithmetic,
consume every selected content sample once, preserve the last frame and never
produce empty bins for a nonempty source. A zero-length verified prefix produces
an explicit empty-source state. Compute each channel independently, initialize
peaks from the first content sample (not zero), and preserve DC offsets and
values outside ±1. No normalization, downmix, resampling, gain, fades or clipping
is applied to raw bins. A bounded visual amplitude scale may contain those
values, but must label the scale; amplitude above full scale is not silently
clamped out of the summary.

Reuse current format support: RIFF/WAVE PCM16/24/32 or float32; native S16LE,
S24LE or F32LE, interleaved or planar. The coordinator plans internal decoder
helpers with an explicit channel limit: waveform WAV supports up to 32 channels,
native PCM supports its existing descriptor ceiling of 16, and public playback
wrappers retain their existing eight-channel limit. Wide-source inspection must
not silently broaden playback acceptance. Channel labels are
indices unless actual channel-layout metadata exists; do not invent speaker
positions. Extensible/compressed WAV remains unsupported. Reject invalid sizes,
partial samples, contradictory format metadata, malformed headers and any
non-finite PCM in the selected source's full verified content, even outside the
current trim. Failure yields no success-shaped partial waveform.

Native `ChunkKind` loss records contribute **gap frames**, not zero samples.
Each bin has shared content/gap counts; `content + gap == bin width` after complete
coverage validation, and a channel peak is absent exactly when that bin has no
content. Mixed bins retain real content peaks while visibly marking missing
coverage. Keep an explicit distinction between recorded zero-valued silence and
missing data. Do not draw continuity through fully missing bins. Verify the
complete selected prefix and the sealed index, including normal chunk integrity;
a missing/corrupt content record is an error, not an invented declared gap.

For recovered takes, the unknown tail is **not** a known interval with invented
duration. Draw the verified prefix only, with an unknown-end marker/caption.
Retain the fact even when the current clip ends earlier. No unknown tail, raw
event ordinal or host timestamp is mapped to an estimated audio duration.

### Initial explicit limits

| Resource | Initial limit/policy |
| --- | --- |
| Legacy asset | Existing 30 MiB ceiling, checked before BLOB allocation and checksum |
| Complete native object | 64 MiB physical/registered ceiling before full authentication, including unrelated streams and retained tails |
| Selected source domain | No arbitrary duration cap; exact checked source range, native end within signed journal bounds |
| Selected content work | At most 32 × 1024 × 1024 decoded channel samples, checked arithmetic before scanning |
| Sealed object verification | At most 65,536 complete-object manifest chunks, including other streams, preflight before the existing full verification |
| Channels | WAV 1–32; native 1–16; no implicit folding |
| Resolution | Adopted UI request 1024 bins; request 1–1024, reduced to frame count for short sources |
| Successful reply | Adopted 2 MiB in-memory typed summary ceiling for the current repeated-coverage DTO, including allocated bin/Vec capacity and identity overhead; no JSON serialization on UI channels |
| Queues/cache | One admitted request, one completion, one displayed summary; no retained full decoded PCM |
| Native temporary payload | Existing 1 MiB audio chunk plus bounded header/metadata and accumulators |
| Cancellation checks | Before/after reader operations, each 64 KiB physical hash read, between chunks and at most 4096 decoded channel samples apart |

These are adopted initial configuration ceilings, not timing measurements. The accepted
64 MiB physical cap bounds the existing sealed-validation interval but does not
make it interruptible or bound external-drive wall time. Enforce limits before
allocating/repeated decoding, not after accumulating bins. Legacy BLOB checksum
and bounded SQLite reads may also complete before cancellation is observed.
Existing source verification can scan other sealed streams while authenticating
the object; only the chosen audio stream contributes peaks.

The coordinator rejected a fixed 120-second waveform limit: duration does not
measure work for a source containing long gaps. Process gap ranges with interval
arithmetic, never one iteration per missing frame. Content/object limits still
exclude many long real recordings. They must not be confused with the separate
long-recording storage design or qualify 30-minute waveform review. An oversized
source remains safely saved and editable; its waveform is unavailable with an
explicit limit message. Do not truncate it, downsample by skipping transients,
or label a partial overview “full source.”

The initial DTO repeats four u64 coverage fields and two optional f32 extrema
per channel/bin. At 32 × 1024 bins that is approximately 1.5 MiB plus Vec/identity
overhead; a 256 KiB ceiling would contradict the chosen shape. The coordinator
adopted the explicit 2 MiB in-memory ceiling. Shared coverage could be separated
from per-channel peaks if a smaller reply is required later. This is not a
serialized-JSON size claim.

The initial review required `WaveformData::validate` to bind the source key:
WAV SHA must equal the selected asset stem, source origin must be zero, and gaps/
unknown-tail must be absent; native rate must equal the request/source rate,
channels must remain within 16 and range endpoints within journal limits. Verify
total content-frame counts times channels against the sample budget (use the
authenticated selected prefix's `recorded_units` for preflight, then independently
reconcile counts while scanning). Validate
finite/nonnegative legacy settings and native window/placement/fade invariants
for both saved and original projections, with native windows inside the verified
source range. Legacy metadata/decoded-extent mismatch remains an explicit overlay
warning. On 2026-10-20, before the source gate, inspection confirmed these
identity, settings, work-budget and actual Vec-capacity checks in the root-owned
DTO, in addition to bin partitions, channel coverage parity, finite ordered
extrema and absent peaks for gap-only bins. These design findings are resolved
in source; test and GUI acceptance remain separate evidence.

## Saved, original and local-draft overlays

Preserve the prototype's useful workflow: explicit Show/Hide, full-source
context, selected-window/fade indication, visible unsaved preview, fallback to
the saved window on invalid input, locked-track review and stale-load isolation.
`public/clip-waveform.js` is a behavior reference, not the native decoder or
contract: it combines channels and zero-biases extrema (lines 11–14), and extends
the axis to declared duration (line 21). Native per-channel/raw-source rendering
should deliberately avoid those numerical shortcuts.

Draw saved and permanent-original boundaries with distinct styles. Original
means the retained first clip settings on the same immutable source, not the
current editable values or a reconstructed time-alignment claim. Keep null,
omitted and explicit zero fades distinct in the inspector even where their
envelopes look identical. Draw envelopes separately from peaks and label the
curve (linear/equal-power). Use the renderer's frame/end convention; finite
sampling of an equal-power display curve is a visualization approximation, not
rendered preview audio. Raw source bins never change when a fade slider/text
field changes.

**Fade constraint correction, 2026-10-20 before the source gate.** Native
`TakeClip::validate_current` requires checked
`in_frames + out_frames <= source_window_count`; `original_view` applies the
same validation to a retained native snapshot. Native overlap is therefore
invalid, with no tolerance. Legacy validation uses finite seconds, a positive
window of at least 0.001 seconds within its declared duration, and the existing
`fade_in + fade_out <= window_seconds + 1e-9` tolerance. The two fades and the
window endpoints are independently rounded into frames, so a valid legacy
seconds view can produce a one-frame fade overlap. Preserve that compatibility
case rather than applying native integer validation to legacy overlays. The
renderer multiplies applicable linear factors and only then applies
`sin(product * PI/2)` for equal power. Neither that gain law nor the prototype's
fade rescaling authorizes an overlapping native saved/draft range.

Apply only the small typed Keep/Set/Remove patch to a bounded saved-settings
projection. Do not clone a document, create editSource, upgrade a schema or
change loop length. Keep preserves exact JSON spelling/presence and Remove uses
the existing declared default; waveform rendering must not rewrite either.
Legacy seconds-to-frame drawing follows the current finite conversion and
rounding convention, is labelled approximate, and leaves the exact decimal
edit text untouched. Native source windows and frame lengths remain exact
checked integers. A local placement-only edit changes placement annotation,
not source geometry. Loop extent does not redefine the source axis.

Validate drafts as a combined window/fade proposal, then compare against returned
authenticated bounds. Incomplete/reversed/overflowing/out-of-bounds drafts show
the saved overlay and a clear local warning; preserve their text and allow the
existing explicit Save/rejection flow. Never clamp a draft into a different
candidate. If saved legacy duration/window metadata differs from real decoded
extent, show the actual source plus an explicit out-of-bounds/declared-duration
warning rather than extending a fake waveform. A view crossing a declared gap
may be shown with its gap warning; it does not assert that the existing playback
renderer can play the loss region.

Show/Hide is a read-only inspector action, not another dirty draft family. It
must not clear retained note/clip/metadata failures, replace generic retry, run
continuations or synthesize Save/Discard. Keep Stop/Panic responsive. Avoid new
background source scans during capture/finalization/attachment; viewing an
already prepared summary remains read-only. Define other audio/storage admission
guards explicitly rather than reusing a mutation guard that blocks Hide.

## Independent acceptance plan

1. Decoder/bin fixtures: unequal stereo channels, positive/negative DC, transient
   in the final frame, 1/2/1023/1024/1025 frames, requested bins greater than
   frames, negative PCM24, PCM32 extremes, planar/interleaved parity, values
   outside ±1 and non-finite content outside the selected trim. Check exact
   source immutability, true min/max and all-frame coverage.
2. Native identity/coverage fixtures: nonzero initial source frame, multiple
   audio streams, raw Events rejection, gaps at/bin-crossing boundaries,
   gap-only bins/source, recovered unknown tail and mixed valid/bad retained
   tails. Wrong take/manifest/stream/rate, missing/tampered owned file and
   malformed selected chunk must not yield a plausible success plot.
3. Budget/read-only tests: no create/migrate, wrong schema/WAL/symlink policy,
   bounded oversized assets/objects, checked frame×channel and bin arithmetic,
   limits before allocation/authentication, fixed summary ceiling, all catalog
   rows/file bytes/original journal bytes unchanged after success/error/cancel.
   Selected-source review must not authenticate unrelated project media.
4. Worker fault tests: held decoder/hash/read, Hide and immediate Show, repeated
   cancellation without extra admission, late success after Hide, track/project
   switch-away-and-back, save revision change, source replacement and stale
   worker generation. Credit remains held until the actual terminal is consumed;
   dropped UI does not join; one terminal cause survives full/disconnected paths.
5. UI headless tests: exact project/address identity, no source I/O in paint,
   bounded per-channel drawing and accessible text, saved/original/null/omitted
   overlays, typed draft fallback without text loss, large-number Keep behavior,
   source-axis independence from arrangement, locks, dirty-family/retry
   preservation and responsive Hide/navigation while the worker is held.
6. Integrated qualification: source gates first, then the same newly built
   native package on isolated synthetic actual WAV/native PCM fixtures. Bind
   real UI selection/overlays to exact raw-bin/source checks, save/reload,
   Undo/Restore, complete archive/copy and restart without source changes.
   Existing prototype or `216d` clip evidence does not qualify the new waveform.

This packet requires no microphone, native playback, inference or new codecs.
Waveform viewing is not waveform dragging, zoom, sample editing, draft audition,
alignment calibration, listening acceptance or completion of milestone 2.
