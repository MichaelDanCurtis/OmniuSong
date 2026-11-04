# Waveform lane clipping addendum

2026-10-20. This is an owner diagnosis and bounded correction of
`waveform_ui.rs`, not an independent review of the reviewer's implementation.
It supplements the earlier [integration review](RUST-WAVEFORM-UI-INTEGRATION.md).
The coordinator reported the initial `1b7076…` package attempt and performed the
actual UI actions; this reviewer inspected the retained screenshots and source
without running Cargo, formatting, GUI, devices or catalog operations.

## Observed failure

Both retained images show the source lane near the bottom of the central
inspector. The positive half and centre line are visible, while the lower half
extends beyond the central clip boundary above the status panel. Repeated outer
scrolling did not expose the complete lane. A screenshot or AX label establishing
that a waveform exists therefore did not establish usable full-lane visibility.

| Coordinator artifact | SHA-256 |
| --- | --- |
| `.runtime/rust-waveform-ui/06-legacy-source-graph-scrolled.png` | `dba5f975cfd76bf4df5bd880d985cf668f6ae6cb54d206c4b950b33670473008` |
| `.runtime/rust-waveform-ui/07-legacy-source-full-lane.png` | `29c7dd84f54a762b2298f49ff4e493a671dfb9c8b19c64adfd97f0fc39e0f33c` |

The original attempt remains failure evidence. This addendum does not relabel it
as a successful waveform walkthrough or qualify a corrected package.

## Source diagnosis

The installed egui 0.37.0 `containers/scroll_area.rs` explains this geometry:

- `ScrollArea::new` defaults `min_scrolled_size` to 64 points per axis.
- `begin` starts from the remaining parent space, applies `max_height`, and then
  raises a scrolling viewport only to its configured minimum. `max_height(340)`
  is a ceiling, not a promise to reserve a complete channel row.
- `show_rows` advertises total virtual content separately from that viewport.
  `end` advances the parent cursor by the viewport's outer rectangle.

The waveform nested scroll area had only `.max_height(340)` and a hardcoded
102-point virtual row. Near the end of the outer inspector it could receive a
64-point viewport, which is shorter than its channel label, item spacing and
78-point plot. The outer inspector then accounted for that short viewport and
could reach its own bottom while the plot remained clipped inside the inner
scroll area. Inner scrolling could change which portion is exposed, but requiring
that hidden interaction to see one channel is unsuitable.

This is a layout-allocation issue. It does not indicate truncated source audio,
missing waveform bins, incorrect negative amplitudes or a renderer change.

## Bounded correction and regression

The authorized source change is limited to the channel viewport in
[waveform_ui.rs](../../crates/omniusong-app/src/waveform_ui.rs):

1. Retain the 78-point plot height as a named constant.
2. Measure one row as Small text height plus item spacing plus plot height.
3. Use that row height for both `min_scrolled_height` and `show_rows`; keep the
   340-point maximum and virtualized channel range.

This reserves enough outer-scroll content for a full channel while retaining
inner scrolling and bounded painting for up to 32 WAV channels. The row argument
excludes the spacing that `show_rows` adds between rows, matching egui's API.
No source, settings, request identity, edit guard or projection semantics change.

The focused owner regression
`nested_inspector_at_bottom_exposes_a_complete_lane_and_retains_channel_virtualization`
constructs a real egui nested scroll layout with a 320-point viewport and
900 points of preceding inspector content. It checks the outer scroll offset is
actually at its bottom and compares painted 78-point plot rectangles against
their `ClippedShape.clip_rect`, requiring a whole positive/negative lane to fit.
It covers one and 32 channels with default and enlarged Small text, and bounds
the number of painted channel rows. This checks geometry rather than merely AX
node presence. The test was written but not executed by this reviewer.

The pre-central-format corrected source SHA-256 is
`8306ebdf73008ad475accd2763d606c681af3307c35ea3cc97e520349eb2b8ae`.
`git diff --check` passed. Central tests/Clippy/build and a fresh actual full-lane
walkthrough remain necessary before accepting the correction. The coordinator
retains the initial package, screenshots and source/catalog preservation evidence.
