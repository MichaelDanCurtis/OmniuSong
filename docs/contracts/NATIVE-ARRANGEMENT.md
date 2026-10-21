# Native arrangement and placement

This packet adds a metadata arrangement view and local placement controls to the
Rust app. It uses the existing `UpdateClip` operation, clip draft, checked Save,
history, original-settings snapshot, archive and exact retry contract. It adds
no store schema, IPC command, source rewrite or fourth draft family.

## Timing

Notes and legacy audio use 960 ticks per beat. A native audio clip keeps its
signed integer frame position and immutable sample rate; its source range is a
separate absolute unsigned frame clock. An event range counts records and is
drawn as a durationless marker. A note summary on an audio track remains visible
as retained read-only content. Unknown extension fields are never interpreted
as notes or audio.

The common axis is display-only fixed point at one billion units per tick.
Frame projection uses the exact binary tempo ratio, outward interval rounding
and integer viewport subtraction before conversion to pixels. A display value
is never round-tripped into saved placement. A gesture starts from the exact
integer placement and applies a checked delta. Native deltas may exceed i64;
only the final position and its source-window end must fit the durable range.
Legacy moves outside 0..1,000,000,000 ticks are rejected. Explicit snap options
are Off, beat (960 ticks), and sixteenth note (240 ticks); musical grid rounding
is half away from zero. A no-displacement or unchanged-placement gesture keeps
the exact existing draft, including Keep/Remove modes and number spelling.
The displacement rounds into the target's integer unit first, then adds to the
exact baseline, then snaps that integer candidate to the musical grid. Snap Off
nudges use one target unit. Snapped nudges choose a strictly earlier/later grid
position after frame rounding, skipping a grid point that rounds back to the
current frame.
If the displacement rounds to zero target units, it preserves the placement
before applying snap. Pointer jitter cannot quantize an off-grid saved value.

Legacy audio width is a declared-seconds estimate: `((duration * BPM) * 960) / 60`
uses binary floating-point evaluation; its exact binary result rounds outward
to fixed point and adds to the exact integer start. The core's authoritative
loop-extension calculation sums start and duration in floating point before its
final extent check. These distinct estimates need not have bit-identical display
endpoints. Neither authenticates the decoded source or changes stored timing.

## Interaction ownership

Selection is an existing guarded local choice. Selecting an unselected clip
cannot move it in the same gesture. An already selected editable audio clip can
show a transient drag ghost; release stages one typed placement into its clip
draft. Save and Discard remain explicit. Earlier/Later controls use the same
typed route. The exact inspector remains authoritative for integer entry.

A gesture captures project identity, revision, full clip context and transform.
Changed selection, revision, source, draft, transform, availability, focus or
pointer presence cancels the ghost. Escape and release outside the viewport
cancel too. No gesture survives a pending selection continuation. The final
local entry point checks the captured context and saved source identity again.

Same-target trim/fade changes survive staging. Invalid fields remain intact;
the timeline cannot replace them with a rounded guess. Locked sources remain
inspectable. Metadata/note drafts, retained uncertain/rejected musical edits,
pending modals, storage loss, audio/recovery work and close guards block moves.
All combined source-window, fade, timeline, schema-upgrade and revision checks
remain authoritative on Save. No implicit loop extension occurs.

## Bounded view

The view projects only validated metadata and caches primitive summaries by
project ID/revision. It performs no source decoding, authentication, disk I/O,
or full-document serialization in paint. The document bounds permit at most 64
tracks and 16,384 declared native take clips in total. Projection and individual
visible shapes have explicit bounds; dense content uses an honest summary and
the exact selector. Track and item identity use complete validated keys, never
truncated display strings. Rows are virtualized and measured at full height.

Zoom and pan do not change music or the saved loop. Negative positions and
out-of-loop content stay navigable, independent of the renderer's current
120-second playback bound. A loop marker is contextual, never a crop or implied
render capability. This packet does not supply waveform handles, time stretching,
MIDI expression editing, inference, physical-device or listening acceptance.

## Acceptance

Pure checks cover binary tempo projection, huge origins, signed frame extremes,
outward boundaries, snapping and rejected placement. Integration checks cover
no-op context preservation, same-target fields, stale context/source/revision,
other draft and retained-card guards, selection, and existing save/history retry
paths. Native UI acceptance must separately observe selection, a staged move,
Save/Discard, unchanged immutable sources, layout, and process restart. A passing
source/build gate alone does not imply that the native walkthrough passed.
