# Native arrangement architecture review

Reviewed on 2026-10-20 against baseline `a1241b94edbd3e50bd9a3e41ae5a7089daf90863`.
The accepted baseline is the `9b2fb28c…` waveform package and its separate
517-passed, zero-failed, two-ignored source gate. Those results do not qualify
the arrangement proposed here. This review used source inspection only; it did
not run Cargo, open the live catalog, operate the GUI or open an audio device.

## Delivery decision

Add a real, project-wide arrangement overview above the existing inspectors.
Show saved legacy audio, native audio, note activity and explicitly placed raw
event sources. Select a clip in the overview, inspect its current/original source
window in the existing inspector/waveform, and move its arrangement placement
with a second, separate pointer gesture. The gesture produces an unsaved,
reviewable placement in the existing `ClipEditor`. Only its existing Save path
submits `UpdateClip`; Restore, Undo/Redo, conflict handling, exact retry and
archive semantics remain the existing paths.

The coordinator adopted a musical fixed-point axis, typed placement values,
absolute musical snapping, captured-base dragging and exact no-op preservation.
The shared app-local declarations now exist in `arrangement_types.rs`:
`TickFp(pub i128)`, `TICK_SCALE = 1_000_000_000`, `Viewport { origin, span }`,
`PlacementValue::{LegacyTick(u64), NativeFrame(i64)}` and
`Snap::{Off, Beat, SixteenthNote}`. A sixteenth note is **240 ticks**, not one
sixteenth of a beat. These are window-local values, not a new project schema,
durable operation or IPC protocol.

The first delivery should include a musical ruler, horizontal pan/zoom, Fit all,
Fit selected, exact clip selection and placement review. Native frames remain
signed, source-rate-specific integers. Notes and raw events are read-only in
this overview. Note editing in the existing piano roll remains available.
Trimming/fades remain in the inspector. Multi-selection, clip creation/deletion,
track moves/reordering, stretch, resampling, note dragging, automation,
expression editing and event playback remain explicit later arrangement work.
No new seek, loop-playback, playhead calibration or draft-audition claim follows
from drawing the arrangement.

## Why the existing boundaries matter

* `ProjectDocument::update_clip` clones and validates a complete document
  (`core/src/clip_edit.rs:225` onward). It is appropriate for a deliberate Save,
  not for pointer motion or paint. `ClipDraft::patch` already parses bounded
  fields without replacing untouched JSON numbers.
* `main.rs` already has three draft families, both request and dispatch guards,
  and captured note/clip contexts. Adding a fourth arrangement draft or retry
  queue would create inconsistent navigation and recovery decisions.
* Native `AudioFrames` source ordinals, signed arrangement frames and their
  sample rate are distinct. `Events` counts are event ordinals; they do not
  provide a duration. `TakeClip::original_view` also keeps the immutable rate
  and source reference separate from original settings.
* The core permits 64 tracks, 20,000 notes per legacy track and **16,384 native
  clips in the whole project**, not 16,384 per track
  (`native.rs:284–295`). The schema-2 metadata ceiling is 8 MiB. Source lengths
  need not fit the loop or playback preparation's 120-second ceiling.
* The renderer converts legacy start ticks and trimmed seconds using output-rate
  rounding, while native placement uses exact frames at the source/output rate
  (`render.rs:228–234`, `456` onward). Unsupported resampling, gaps and expression
  remain playback errors. A visible metadata rectangle is not media authentication
  or proof that playback is supported.
* The prototype's `public/app.js:99–114` divides everything by the declared loop,
  enumerates every bar and hides the audio rectangle when a track has notes.
  Those are unsuitable native rules. Its automatic save-on-navigation and
  128-bar restriction also do not carry into the native design.

## Ownership and minimum interfaces

| Module | Owns | Must not own |
| --- | --- | --- |
| `app/arrangement_types.rs` | Shared app-local scalar, viewport, snap and placement declarations | Schema, service commands or source resolution |
| `app/arrangement_math.rs` | Checked tempo ratios, time projection, viewport geometry and drag/snap arithmetic | egui, storage, document caches or mutations |
| `app/arrangement_ui.rs` | Bounded saved-metadata projection/cache, viewport, selection affordances, draw/hit geometry and one temporary drag ghost | Durable drafts, operation IDs, file I/O, audio callbacks |
| `app/clip_ui.rs` | Existing complete clip draft and `stage_placement` | Arrangement-specific retry semantics |
| `app/main.rs` | Authoritative selection, draft-family/state guards and action dispatch | Per-pointer service admission |
| Existing core/store/session | Save validation, source authentication, revision/receipt transaction, history and archives | Screen-space geometry |

The minimum math interface proposed to the implementation owner is:

```rust,ignore
TempoRatio::from_bpm(bpm: f64) -> Result<TempoRatio, String>;

enum ClipTiming {
    Legacy { start_tick: u64, duration_seconds: f64 },
    Native { position_frame: i64, frame_count: u64, sample_rate: u32 },
}
// PlacementBasis carries the same effective start/count/rate information.
project_interval(timing: ClipTiming, tempo: &TempoRatio)
    -> Result<AxisInterval, String>;
Viewport::new(origin: TickFp, span: TickFp) -> Result<Viewport, String>;
viewport.project_interval(interval: AxisInterval, width: f64)
    -> Result<Option<PixelInterval>, String>;
drag_candidate(basis: PlacementBasis, delta: TickFp,
               tempo: &TempoRatio, snap: Snap)
    -> Result<PlacementValue, String>;
```

The math owner's internal model names may differ; the shared scalar types and
rounding rules are fixed. Errors are local diagnostics, not session-service
failure classifications. A `PlacementBasis` for a valid local native trim uses
that draft's effective count. It must not silently use the saved count instead.

The UI returns finite actions, with no storage path or arbitrary JSON mutation:

```rust,ignore
ArrangementAction::SelectClip(ClipSelection);
ArrangementAction::SelectTrack(String); // exact track identity
ArrangementAction::StagePlacement {
    expected: ClipContext,
    value: PlacementValue,
};

ClipEditor::stage_placement(
    &mut self, doc: &ProjectDocument,
    expected: &ClipContext, value: PlacementValue,
) -> Result<(), String>;
```

`main` routes selection through the existing guarded selection path and routes
staging through local `Action::StagePlacement`. Neither stage nor selection
creates an operation ID. Save does that only when the user submits the reviewed
draft. Selection of a clip in another track must also update the active track
by exact ID and clear the opposite clean editor selection; it must retain any
failure evidence. The same rule applies to note selection so the new view does
not create two different notions of the selected track.

## Exact source meaning and projection rules

The overview reads only schema-owned paths of a validated `ProjectDocument`.
Its cache contains small timing/identity values, not raw source metadata,
expression arrays, original JSON snapshots or serialized documents.

| Material | Arrangement span | Source information retained separately |
| --- | --- | --- |
| Legacy audio | Start `startTick` (default zero); duration from effective `trimEndSeconds − trimStartSeconds`, with trim defaults zero/declared duration | Exact asset identity, stored JSON Number spelling/presence and original settings remain in the document |
| Native audio | `[position.frame, position.frame + range.count)` at `position.sampleRate`, then projected to musical ticks | `range.start` is an absolute source-frame ordinal; take/manifest/stream/rate never change |
| Notes in `notes` or `audio` tracks | Exact tick start/end, or a clearly labeled read-only activity span/count | Repeated pitch/overlap is not deduplicated; input, expression and raw records are untouched |
| Native `Events` clip | A point marker at its signed arrangement position/rate | Event start/count displayed as ordinals; no invented end, source alignment or note estimate |
| Recording-track extensions | No inferred musical geometry | Fields named `notes`, `audio`, `takeId` or `recording` outside the declared native paths stay opaque |

Known notes on legacy audio tracks must have their own read-only activity marker
even when an audio rectangle exists. A note-span summary is labeled as a span of
N notes, not drawn as evidence of continuous sound. Legacy `inputCapture` can be
reported as retained raw metadata; it is not a new native recording stream.

Use the **saved** tempo for all saved geometry. A local metadata tempo draft is
not applied to the arrangement and blocks placement edits through the foreign
draft guard. Musical note/tick placements remain at their ticks across a saved
tempo change; native frame placements stay at their seconds and therefore move
relative to musical ruler marks. Legacy audio starts remain musical, while its
trimmed duration remains seconds. No time-stretch is implied.

Legacy geometry uses the same finite seconds-duration conversion order as
the clip extent contract, but preserves its integer start separately:

```text
duration = trimEndSeconds - trimStartSeconds
durationTicks = ((duration * tempo) * 960) / 60
startFp = startTick * S
endFp = startFp + ceil_exact_binary(durationTicks * S)
```

The coordinator explicitly selected this order. The authoritative core loop
extent instead sums `startTick + durationTicks` in f64 before its bar ceiling.
Those estimates can differ by a few fixed-point units at a large tick origin;
the arrangement does not use its drawn endpoint to decide loop extension. For
example, start 1,000,000,000 ticks, duration 0.123456789 seconds and 119.25 BPM
produce fixed endpoints 1000000235555553412 in the view and
1000000235555553437 when the core's floating end is subsequently projected.
Both retain the original stored values. This distinction is not a changed
rendering rule or a claim of audible timing difference.

The displayed body is an estimate based on declared metadata. It is not the
decoded source length or exact output-buffer endpoint: the renderer rounds the
two source endpoints and the start separately at the actual output rate. Do not
change that renderer to make screen geometry appear exact. The waveform remains
the explicit authenticated source view. A declared/decoded mismatch does not
authorize a source rewrite.

Native geometry uses `E = checked(position.frame + count)`, before conversion.
Negative placement remains visible as preroll; crossing zero does not shift
`range.start` or shorten the stored clip. Current playback skips negative
arrangement frames without rewriting the view. Native original settings are
projected from their own retained range/position, with the same immutable rate.
Optional absent versus null fades and all opaque snapshot extensions remain
unchanged. Native fades obey checked `in + out <= count`; legacy fades retain
the existing seconds tolerance. The arrangement need not redraw detailed fade
envelopes already shown by the source waveform.

Current bars are a loop guideline, not a clip limit or visible-domain bound.
Show out-of-loop material. `PreserveLoop` stays the default. Dragging never
extends bars or upgrades schema implicitly. The existing explicit ExtendLoop
and schema-upgrade choice remains in the inspector; Save applies the existing
exact native/floating legacy extent rules. An imported long clip can remain
visible/editable with PreserveLoop even when its extent cannot be represented
as an extended loop.

## Geometry arithmetic and bounds

Define `S = 1_000_000_000` fixed units per tick. `TickFp` is **display-only**.
Store each original native frame/count/rate and legacy tick value separately.
Never recover a saved placement or source window by converting a pixel or
`TickFp` back into the document during paint.

For validated tempo 30–300, decode its IEEE-754 value exactly as `m / d`, with
`m = 2^52 + fraction_bits` and `d = 2^(52 − exponent)`. The unbiased exponent is
4–8. No decimal reparse or rounded BPM string is involved. A native frame `f`
has musical position:

```text
ticks(f) = f * m * 960 / (sample_rate * 60 * d)
```

The unsigned magnitude numerator fits below `2^126`. Compute its quotient and
remainder before multiplying by `S`; multiplying the entire numerator by `S`
would overflow. Use signed floor for interval starts and signed ceiling for
ends. This creates outward geometry with error below `1/S` tick per boundary.
Convert the legacy floating duration to fixed point from its exact finite binary
value with directed rounding, rather than claiming that a rounded `f64 * S` is
an outward bound. Add its exact integer start afterwards. The legacy estimate
has its separately stated floating-point semantics above.

The absolute display domain is:

```text
AXIS_ABS_TICKS = 6_000_000_000_000_000_000
-AXIS_ABS_TICKS*S <= TickFp <= AXIS_ABS_TICKS*S
```

This covers every valid native endpoint: the largest magnitude is bounded by
`abs(i64::MIN) * 300 * 960 / (8000 * 60) < 5.535e18` ticks. It also covers all
legacy/note bounds and `u32::MAX` bars. It is not a new musical limit. Validate
positive viewport span and both endpoints against this domain. A full-domain
delta has whole-tick magnitude below `u64::MAX`.

Before any pixel conversion, intersect/cull intervals using integer `TickFp`,
subtract the viewport origin as `i128`, and only then convert the local result
to `f64`. Clamp only draw geometry to the canvas, never the source interval or
proposed edit. A minimum-width pixel marker for a tiny clip is a hit affordance,
not a changed duration. Handle zero-width raw-event points separately. Reject
NaN/infinite dimensions/deltas. A rectangle wholly outside the view must not
produce giant pixel coordinates or be silently selected at a clamped edge.

Recommended initial viewport is a short musical window around zero, with Fit
all and Fit selected available. Fit all uses the union of saved material and
the loop guideline; it does not enumerate every bar. Fit selected may include
its original and valid local draft. Keep a finite positive minimum span, e.g.
one tick; exact values remain available in the inspector below pixel resolution.
Use adaptive ruler spacing with at most 256 marks. Negative time labels identify
preroll; positive bar/beat labels use the current fixed four-beat convention.
Do not claim a time-signature editor or tempo map.

## Drag and snap arithmetic

Capture the effective exact placement, source count/rate, saved tempo ratio,
viewport transform and complete `ClipContext` once at gesture start. Pointer
movement produces a bounded `TickFp` delta. Every candidate is computed from
that captured baseline plus the latest total delta, never accumulated frame by
frame. This prevents cumulative rounding drift.

For native conversion of a fixed-point delta, do not form
`deltaFp * rate * 60 * d` directly. The following decomposition covers the entire
display domain without a new arithmetic dependency:

```text
t = abs(deltaFp)                         // unsigned magnitude
q = t / S; r = t % S
A = rate * 60 * d; B = m * 960
aq = A / B; ar = A % B
x = q * ar
base = q * aq + x / B
z = (x % B) * S + r * A
den = B * S
base += z / den
round base upward iff 2 * (z % den) >= den
restore the sign                        // half ties away from zero
```

Here `q < 2^64`, `ar < 2^63`, `x < 2^127` and `z < 2^105`; checked `u128`
operations suffice. Keep the signed resulting frame delta in `i128`. Moving
from `i64::MIN` to near `i64::MAX` can have a delta greater than `i64::MAX` while
both placements are valid. Add the baseline in `i128`, then check the resulting
`i64` placement and signed end with the effective count. An intermediate i64
conversion must not reject a valid target. The same rule applies to signed
legacy tick deltas before checking the final range 0–1,000,000,000.

Snap Off rounds the free candidate only to its own legal integer unit. For
Beat or SixteenthNote, use an absolute grid rooted at tick zero, with step
`G = 960` or `G = 240`. Legacy uses nearest integer grid multiple. Native uses:

```text
k = round_away(candidate_frame * m * 960 / (rate * 60 * d * G))
snapped_frame = round_away(k * G * rate * 60 * d / (m * 960))
```

The order is intentional: convert the captured total fixed-point displacement
to an integer frame delta using half-away rounding, add it to the exact baseline,
then snap that integer candidate to the musical grid and round the grid back to
a frame. Do not substitute snapping the continuous pointer coordinate before
the integer candidate; midpoint behavior would differ.

Use the same quotient/remainder method for the second conversion. Snap chooses
the musical grid then the nearest representable source-rate frame. At fractional
tempo a frame may not lie exactly on the grid; show the resulting exact frame
and do not promise exact musical alignment finer than one source frame. Ties
are away from zero, including negative preroll. An out-of-range candidate or
end is visibly invalid; it is not clamped into a different accepted edit.

The coordinator also accepted `nudge_candidate` for explicit Earlier/Later
buttons. Snap Off means exactly minus/plus one tick for legacy or one frame for
native. With snapping, it chooses the strictly previous/next grid point, rather
than rounding back to the current grid point. Apply the same native nearest-frame
conversion and final bounds. The effective candidate must change in the requested
direction; never label an unchanged rounded result a successful nudge.

Zero movement, movement below the integer-unit threshold, or a final candidate
equal to the captured effective placement is a no-op **before** applying snap
to an already off-grid baseline. It must preserve Keep/Remove/Set, raw text,
timeline policy and the complete context serial. A gesture returning to its
start cannot silently quantize a performance. Snap selection itself does not
change a clip. Placement staging changes only the placement field to Set with
its exact decimal integer; source trim/fades/Number spelling remain untouched.

## Selection, drafts and recovery

Use a select-first, drag-second interaction. A click on another clip sends only
its exact `ClipSelection { project_id, address }` through `main`. A later gesture
can begin only after that selection is installed. Resolving a Save/Discard/Cancel
modal must not resume an old pointer displacement against a newly installed
document or target. All widget IDs include project, exact track and exact clip
identity; copied projects deliberately reuse clip/track IDs and must stay
separate. Array index, clip name, drawing order and quantized grid cell are not
edit addresses.

During dragging, paint a ghost separately from saved and permanent-original
outlines. Do not modify the draft on each mouse move. On release, emit one
StagePlacement action with the captured complete context. Main checks ready,
connection, busy/audio/recovery/modal/close state, foreign dirty family, all
unresolved musical cards and the current target before invoking staging.
`ClipEditor::stage_placement` checks project/revision, exact draft/selection/
serial, immutable source identity, declared kind, unlocked track and value unit.
It changes only local placement. Existing source authentication still occurs on
Save; drawing/staging cannot certify a file or turn a source error into a known
semantic rejection.

Valid same-target dirty source trims/fades and explicit timeline policy can be
retained while placement is staged. Invalid draft text remains visible in the
inspector; geometry falls back to saved settings with an explanation and drag
is disabled until its effective basis is valid. No fallback is an authorization
to replace the invalid text. A foreign metadata/note draft must use the existing
guard, never be discarded by selection or movement.

Cancel the temporary ghost on Escape, focus/pointer loss, document installation,
selection/context/source change, busy/modal transition or viewport/layout change
during a gesture. Do not reinterpret old pixels using a new zoom or resized
canvas. No service request or source mutation exists to cancel at this stage.
After release, the resulting clip draft receives the existing Save/Discard/Cancel
navigation and close behavior.

Known and unknown Save failures remain the existing full-intent cards. A retry
uses the same operation ID, revision, complete patch and timeline policy. The
arrangement does not mint a fresh operation or rebase its ghost. Suspended
failures can permit read-only navigation, but still block new musical mutations.
ReviewLatest/NewChoice/ReleaseRejected retain their existing cross-family rules
and metadata-retry context exception. Stale completions must not install an
older document over a newer clip/note/metadata context or run a navigation
continuation. Stop/Panic and waveform Hide remain reachable under their existing
rules; an arrangement gesture cannot acquire their input or lifecycle credit.

## Cache, paint and accessibility limits

Cache known metadata projection once per installed document identity/revision,
and invalidate it on **every** document installation, including a same-revision
reopen. A revision comparison alone is not a proof that a replacement document
is identical. Local draft overlays are small selected-settings projections;
they do not rebuild or clone the project. Viewport changes alter geometry, not
source interpretation or cache identity.

Initial explicit bounds are 64 lane summaries, 16,384 native audio/event records
plus at most 64 legacy audio records, and 32 MiB of accounted cache allocations
(including Vec capacities/owned identity text). Notes can be reduced to exact
read-only spans/counts in each known legacy track, without retaining expression
or every note as a widget. A source or budget diagnostic must be visible;
oversized metadata does not become an empty track or an incomplete model labeled
complete. The existing inspectors remain available.

Virtualize track rows. Paint at most 4,096 individual visible clip/marker shapes
per frame and at most 256 ruler marks. If density exceeds the individual shape
budget, show a labeled dense summary; do not drop material silently or assign
an arbitrary hidden clip to a summary's hit box. Preserve an exact selected clip
outline and use the existing complete clip selector for overlaps/dense material.
Bound display labels and diagnostics; retain full exact IDs for actions. No
opaque metadata serialization, whole-recording deserialization or storage read
belongs in paint. This is a resource design, not a measured frame-time result.

Provide ordinary accessible selection controls and exact text placement in the
inspector, so dragging is not the only path. Any additional nudge buttons route
through the same typed StagePlacement action; do not install global arrow-key
handlers that steal live keys, text input, IME or existing focus cleanup. Locked
and muted sources remain inspectable and labeled; locked clips have no move
handle. Displaying an empty or event-only lane must not arm, record or synthesize
anything.

## Required independent tests and actual acceptance

Pure geometry and staging tests must cover:

1. Native positions/counts at zero, negative preroll, crossing zero, minimum and
   maximum supported rates, fractional tempos, far beyond 128 bars and near i64
   extremes. A valid large move must not fail merely because the direct multiply
   or signed delta would overflow a narrower intermediate.
2. Exact rational outward boundaries, subtraction-before-float and one-frame
   distinctions near a huge origin; full-domain Fit and clipped draw geometry
   remain finite. Use independent integer/rational reference expectations, not
   a second call to the production conversion as the oracle.
3. Absolute 960/240 grids, positive/negative midpoint ties, off-grid no-op,
   returning to the drag origin, fractional-tempo nearest-frame projection,
   candidate/end overflows and invalid pointer geometry.
4. Exact legacy defaults and duration evaluation order; absent fields, raw long
   Number spellings, native absent/null fades and opaque original metadata are
   untouched by paint, selection, cancellation and placement-only staging.
5. Valid same-target dirty trims/fades survive stage; invalid drafts remain
   unchanged. Wrong unit/source/project/revision/selection/serial, locked targets,
   foreign drafts and every retained/uncertain/busy/audio/recovery/modal/close
   guard refuse a stale or unauthorized proposal without service admission.
6. Cross-track and copy-project identity, overlapping clips, clean opposite
   selection clearing without deleting failure evidence, notes on audio tracks,
   recording-extension opacity and Events point semantics. Dense/long content
   remains reachable through exact selectors.
7. Document installation and viewport/focus/pointer changes cancel a ghost;
   post-Save completion/retry behavior continues to preserve newer contexts and
   exact durable requests. Zero/final-unchanged movement preserves the entire
   ClipContext, not just its numeric field value.

The coordinator should then run the complete source gates and build a new
package. Actual UI qualification needs a fresh synthetic mixed-source fixture
with duplicate display names, a saved original snapshot, same-track overlapping
native clips, a negative/native long placement, legacy audio with retained notes,
and a raw-event marker. Hash all existing projects, rows, owned takes and original
prototype files before the fixture and after the walkthrough. No native input
device is needed or authorized.

On the **new exact binary**, verify full-height visible lanes/ruler, coordinate
selection, a genuine pointer drag, disabled locked movement, Beat/sixteenth/Off
review, exact inspector values, original/draft overlays, cancellation, source
range/fade preservation, cross-track guards, Save, Undo, Redo, Restore, complete
archive and restore-as-copy isolation, then clean exit/restart/reopen. Record
which actions were coordinates, keyboard or accessibility activation; an AX
activation alone cannot establish a drag. Compare complete before/after saved
documents, not only screenshots or a changed label. Source take/asset bytes must
remain exact. Keep failed/limited UI evidence with its actual binary boundary.

An offline render comparison can establish the saved placement's effect on the
production render pipeline for a compatible bounded fixture; it is separate from
native output acceptance. Software-loopback/output checks, if the coordinator
chooses them, must preserve the user's no-physical-input restriction and state
their exact scope. Neither the prior waveform gate nor this architecture review
proves new GUI behavior, listening quality, MIDI transport, plugins, long capture,
latency, Windows behavior or completion of the full workstation goal.

## Pre-implementation disposition

The design is implementable without changing source formats, durable operation
semantics, session-service protocol, audio-worker protocol or dependencies.
The consequential decisions are now explicit: original-unit ownership; full-range
checked geometry; absolute snap with negative ties; no-op preservation; no fourth
draft; select-first/drag-second; global native-clip bound; guarded local staging;
Events point semantics; and source/package acceptance kept separate.

Implementation acceptance still requires the tests and source audit above.
Subsequent changes to arithmetic limits, snapshot defaults, staging context or
failure handling require review before an actual native walkthrough is claimed.

## Initial implementation review, before the source gate

The implementation keeps the boundaries above: `arrangement_math.rs` owns pure
arithmetic; `arrangement_ui.rs` caches declared metadata and emits local actions;
`main.rs` admits those actions through the existing guards; `ClipEditor` owns the
only musical draft and changes only placement mode/text. The implemented paint
budget is 256 individual sources per frame, below the proposed 4,096 ceiling,
with an exact selector and visible density notice. Note-span scanning is bounded
by the existing 64 × 20,000-note document limit and runs during cache construction.
This source inspection is not a measured paint-time or native GUI acceptance.

The review identified three consequential corrections before qualification:

- A nonzero pointer delta that rounds to zero target units must return the exact
  baseline before snap. The math owner applied this correction; independent
  cases cover both signs, legacy ticks/native frames and both musical grids.
- Every document installation must invalidate cached primitives and any drag,
  even when project ID, revision and tempo are unchanged. Otherwise a fresh
  editor context could derive a move from the preceding document image. The
  coordinator added the install hook and the UI owner added explicit cache
  invalidation. An independent actual egui action checks saved placement
  0 → refreshed 1,234 → staged 1,235 with no service request.
- Saved, permanent-original and local-draft visibility must be projected
  independently. Culling by the effective draft alone hid a still-visible saved
  or original view. The UI owner corrected the union used for painting, while
  retaining only effective-current geometry as a movable hit target.

The final visible-scroll-boundary correction is also present in the frozen UI:
the stored clipped lane rectangle, not merely the full geometric row, constrains
drag release. The owner regression establishes the visible clip intersection
before a real pointer press, then releases inside the geometric row but outside
that intersection and requires no staged action. Separate pointer cases require
the normal egui drag threshold; a one-pixel click jitter remains inert.

Read-only inspection of the formatted UI, math and coordinator integration
found no additional implementation blocker for this bounded slice. Saved and
original outlines have independent visibility; only effective-current geometry
enters the hit list. Full document-install invalidation is the first action of
`StudioApp::install_document`. Every placement still passes the coordinator's
session/other-draft/failure/close guards and the clip editor's exact captured
context/source checks before changing local placement text. The 16 UI owner
cases also cover selection-before-drag, cancellation, invalid same-target fields,
full native clip identities, density and nested viewport layout. These source
and authored-test observations remain separate from execution results.

The independent integration file now contains 19 synthetic tests covering the
staging/guard/retry paths above, immutable source fields and Number spelling,
raw-event rejection, same-revision cache refresh, exact full-domain movement,
one-frame projection at both signed extremes, and the explicitly chosen legacy
duration ordering. No Cargo, native GUI, device or catalog action was run by this
review lane. These are authored checks; passing results must come from the
coordinator's subsequent gate before this implementation is accepted.
