# Native clip inspector and range-admission review

Review date: 2026-10-20. Scope: the new Rust clip inspector, its integration
with metadata/note drafts, and the narrowly typed source-window rejection.
The governing contracts are [clip UI](../contracts/NATIVE-CLIP-EDITOR-UI.md)
and [clip editing](../contracts/NATIVE-CLIP-EDITING.md).

The examined implementation has no remaining material source-review blocker,
and the final integrated source/headless gate passes. This is not actual native
clip UI, listening, device, recording, Windows or complete Milestone 2 acceptance.
The earlier 85ef6cd7… note-editor package is a separate binary and supplies no
clip-inspector acceptance. This reviewer ran no Cargo,
formatting, GUI or device commands and did not access the live catalog.

## Source and recovery boundaries

- The inspector uses exact project/representation/track/clip identities.
  Stable widget scopes include project identity; row indices are presentation
  positions only. Selection survives reordering and missing identities remain
  tombstones. Event ranges stay read-only.
- Legacy seconds are parsed into `serde_json::Number` directly. Keep and Remove
  remain distinct from explicit numeric values. Saved numbers are borrowed;
  over-limit lexemes are neither copied into editable fields nor truncated into
  a replacement. Native placement, source windows and fade lengths remain
  integer fields. The native window and fade object are each edited atomically.
- Known source fields and permanent original settings have bounded projections.
  The old eager recording/source renderers were removed; the native selector is
  virtualized. No full opaque snapshot or source object is serialized in paint.
  Displayed saved ranges are explicitly not full authenticated source bounds.
- NewChoice checks the retained immutable source identity before applying only
  attempted non-Keep fields to a freshly reviewed baseline. Changed sources,
  absent targets and locked targets cannot silently inherit old trims. Explicit
  release retains the rejected evidence without submitting a replacement.
- Main owns the three-family guards, worker submission and acknowledgements.
  Both editor contexts plus metadata and project/revision are captured for each
  attempt. Stale success cannot install over newer local fields or run the
  old continuation. Exact retry preserves the full original request and takes
  a fresh UI context only after its foreign-draft guards succeed.
- Suspended musical evidence prevents a fresh note/clip mutation and audio
  start. Metadata/navigation may proceed under the documented guards. Stop and
  Panic remain available. Sealed-attachment retry has a separate admission
  predicate from audio start so the active attachment guard cannot disable its
  own retry. Busy Close preserves drafts and warns about window-local evidence;
  closing makes no cancellation or rollback claim.

## Review findings and disposition

1. **Correctable source-range errors previously became uncertain outcomes.**
   `operations.rs` now performs current reference/window/file authentication
   after receipt replay and revision admission. Only an explicit native
   `source_window: Set` interval outside that same authenticated audio prefix
   becomes `EditRejected`. Tuple, unit and rate checks precede that comparison.
   Current and retained-original damage, missing/corrupt files, Keep, Restore,
   receipt failures and SQL failures retain conservative error handling. Normal
   candidate verification still runs before publication. No general resolver
   error or message string is reclassified.
2. **The modal disabled metadata Save-and-continue while a rejected musical
   card was retained.** The corrected enablement allows the metadata family
   while preserving the prohibition on saving an unresolved musical draft.
   Independent note and clip cases require the reviewed Open to occur only
   after the metadata save acknowledgement.
3. **Direct AddNote admission could bypass family guards.** The local dispatch
   branch now rejects any dirty family or unresolved musical intent before
   creating a note draft. The independent case checks metadata, note and clip
   drafts and verifies clean local Add still works.
4. **Releasing one rejected card could erase an unrelated uncertain metadata
   retry.** Cleanup now compares the full retired intent before clearing the
   generic retry. Explicit successful ReleaseRejected may advance only a still
   matching metadata retry's local editor contexts: project/revision, metadata
   draft and both pre-release contexts must all match, no musical draft may be
   dirty, the durable request remains unchanged, and its continuation is cleared.
   NewChoice has no such exception. Symmetric positive cases require the exact
   metadata request to remain usable; a newer clean or dirty clip context must
   still prevent stale metadata replay from overwriting local state.

The source-window helper searches by globally unique native clip identity after
the pure core operation has validated the complete address. This relies on the
existing project-wide recording-clip uniqueness invariant; it does not introduce
an index-based target or infer references from opaque metadata.

## Independent tests and evidence boundary

The nine tests in
[`clip_range_rejection_review.rs`](../../crates/omniusong-store/tests/clip_range_rejection_review.rs)
use fresh synthetic journals with audio source frames `[100,108)`. They compare
complete project/history/operation/take rows and exact original/owned journal
bytes. They cover lower/upper overruns, exact rejected retry, corrected choice,
inclusive allowed boundaries with an exclusive end, Restore, replay before
revision/bounds admission, missing/tampered objects, invalid current and original
views, manifest/stream/take/rate/unit mismatches, Keep-only edits and final receipt
insertion rollback.

The independent
[`clip_editor_integration_review.rs`](../../crates/omniusong-app/src/clip_editor_integration_review.rs)
uses real egui/AccessKit processing with injected bounded session/audio peers.
It exercises exact decimal text, large signed frames, omission/null distinctions,
invalid drafts, identity/reordering/copies, family resolution, stale completion,
retained unknown results, reviewed source changes, retry ownership, busy Close
and output guards. These synthetic views are not authenticated audio objects or
native GUI/device observations. The owner tests also exercise bounded native
selection and source projection.

The coordinator's actual session-worker test
[`clip_edit_service.rs`](../../crates/omniusong-session/tests/clip_edit_service.rs)
checks `EditRejected` with `outcome_unknown: false`, exact retry, a fresh corrected
choice and missing-source uncertainty without changing source bytes.

The inspected historical `.runtime/rust-clip-backend/ui-workspace-fifth.log`
contains **463 passed, 0 failed, 2 ignored across 45 result groups**. It includes
the nine independent range tests and the real session-worker case, but predates
the final modal/Add/release-retry corrections and their additional tests.
`ui-clippy-first.log` and `ui-build-first.log` finish successfully for that earlier
source. These logs do not qualify the later source snapshot below. The earlier
`range-service-first.log` contains a test-compilation failure and is retained as
diagnostic history, not reported as a pass. Final consolidated gate and packaged
UI evidence remain separate from that historical result.

The final `.runtime/rust-clip-backend/ui-workspace-final.log` was independently
read and counted: **469 passed, 0 failed, 2 ignored across 45 result groups**.
It includes **112 headless app tests**, all **24 independent clip UI integration
cases**, all nine independent range-admission cases, and the actual
session-worker range classification case. The modal continuation, direct Add,
both rejected-card release cases and the newer-context negative case all pass.
The coordinator reports exit zero for final formatting and all-target/all-feature
Clippy with warnings denied; the inspected formatting log is empty and the
Clippy log finishes successfully. The final package log records a successful
build and package output. No test/build/device command was rerun by this reviewer.

The newly observed packaged executable is **87,413,920 bytes**, SHA-256
`216d8e1c3760242f13985a727629752a7d753e0b16477ab275899d2ed54f1155`, at
`dist/OmniuSong Native.app/Contents/MacOS/OmniuSongNative`. This is the package
identity for subsequent qualification, not evidence that native clip editing,
audio rendering or restart/reopen has been exercised through its GUI. Actual
UI evidence and any signature verification belong to the coordinator's next
explicit qualification step.

## Examined source snapshot

These hashes identify the frozen source inspected after the substantive
corrections and rechecked against the final gate. The historical fifth gate
does not qualify this later snapshot.

| File | Bytes | SHA-256 |
|---|---:|---|
| `crates/omniusong-store/src/operations.rs` | 13046 | `2f4eca5482487303a59583b7d51d3a87ab162dd0e8a8221cf5eb509a05c9dc4e` |
| `crates/omniusong-store/src/lib.rs` | 34009 | `827a79ecef6f55b968b293fc8f16a03e23edfac626547294987910de0cf77599` |
| `crates/omniusong-app/src/main.rs` | 157578 | `a820c50d30546e706cd89976280ee2c60c9da2c629fc7d22f17680852d03f386` |
| `crates/omniusong-app/src/clip_ui.rs` | 64436 | `7880b046a806f81b567736acbfdd05119bf9f6265dce2de22527d66bec2938a7` |
| `crates/omniusong-app/src/note_ui.rs` | 39094 | `8995f53679f5d8445b72425b1aabeff4c17e94181b5ca72977d9d13581d6c3ab` |
| `crates/omniusong-app/src/audio_ui.rs` | 44942 | `f49c4dc1cff1a17fa11c4960a27895e1ea7fa4d1ce3ada4a36edb0bdcf10f370` |
| `crates/omniusong-store/tests/clip_range_rejection_review.rs` | 19691 | `63c6200dad06a2eb7c687a603a213e04f9174ae856440ee7750bb602624dc7f0` |
| `crates/omniusong-app/src/clip_editor_integration_review.rs` | 52749 | `ac012d43c0c3e1258455a4dde3ae1d5c2d1812d9587ba63f3c7e7ab10367aac3` |
| `crates/omniusong-session/tests/clip_edit_service.rs` | 9048 | `4d7b2a9fe3d2492ba0f40ae35e399532a8e5b9b9a0888023c9f5585c1d16f62a` |
| `docs/contracts/NATIVE-CLIP-EDITOR-UI.md` | 8038 | `c7e770baf7be2cc3c1a17355f4a710ca9e5db5ae8f4f14063cf3c7d0cc224701` |
| `docs/contracts/NATIVE-CLIP-EDITING.md` | 8600 | `e0a96ce6616648a623f24ab9c48a9b70ffa4186708131904d23b778c1b7c55b4` |

| Historical gate artifact | Bytes | SHA-256 |
|---|---:|---|
| `.runtime/rust-clip-backend/ui-workspace-fifth.log` | 52053 | `9882357cc8e486f18364ca9e34bd8d2a7d21349aee040083c6e15ef15ae9ca9e` |
| `.runtime/rust-clip-backend/ui-clippy-first.log` | 498 | `31170b2d09ea1c876222066b520639e5345fa5a957c7cb2243205e5538e582f3` |
| `.runtime/rust-clip-backend/ui-build-first.log` | 613 | `fe2c9601d32d406c27c4849a98d07fdfce432cc70a0ab8b961133b4410441b54` |

| Final gate artifact | Bytes | SHA-256 |
|---|---:|---|
| `.runtime/rust-clip-backend/ui-workspace-final.log` | 52546 | `0a6dfa305127b7615c18373ecf772e1bf383ae464c0b0840a0f0c62c2b30a5a8` |
| `.runtime/rust-clip-backend/ui-fmt-final.log` | 0 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `.runtime/rust-clip-backend/ui-clippy-final.log` | 174 | `0cc43e2cd6bb09345e94ebc646cb7ea6441d96bda39a3f3c7e7baa09e4e4563a` |
| `.runtime/rust-clip-backend/ui-package-final.log` | 589 | `1554340c51947005f27939887edc19afc86befccda3af326d25d895b62e84051` |

Remaining acceptance includes actual clip-inspector interaction with committed
render/archive/source comparisons and same-binary restart/reopen. Waveforms, arrangement dragging,
physical capture/latency, listening and Windows qualification remain separate.
