# Native clip editor UI

Status: implemented, independently source-reviewed and qualified through the
bounded [216d Mac walkthrough](../verification/rust-native-clip-editor-ui.json).
The [469-entry source gate](../verification/rust-native-clip-editor-tests.json)
and actual UI are separate evidence boundaries. The builder retained the earlier
85ef note-editor bundle. Milestone 2 and the full goal stay open.

## Scope and exact views

ClipSelection is {project_id, address:DocumentClipAddress}. LegacyAudio selects
the one implicit audio clip by exact legacy track key; NativeAudio selects a
declared audio-frame clip by strict native track and clip keys. Never use row
indices or source names as targets. Native source take/manifest/stream, clip ID,
range unit and sample rate remain immutable. Events are displayed read-only;
event ordinals are not audio duration. There is no Add/Delete/source replacement.

Show a legacy selection action and virtualized native selection rows across all
16,384 permitted clips. Widgets use exact project/track/clip identities through
scope_id(Id::unique(...)), not a position-dependent push_id. Missing identities
stay missing/tombstoned through history; never select a neighbour automatically.

ClipEditor mirrors NoteEditor lifecycle: context, dirty, unresolved, select,
clear_selection, discard, document_installed, intent, failed_with_draft,
not_admitted, suspend, new_choice, release_rejected, succeeded, failure_ui, ui.
ClipContext includes exact selection, bounded draft and change serial; its draft
is available to the coordinator for failure retention. ClipAction is finite:
Select, BeginClip, Save, Discard, Restore, Retry, Suspend, ReviewLatest,
NewChoice, ReleaseRejected. Main retains common track routing and family guards.
The module's ui method receives the UI, document, exact track key, base
availability, other-family dirty flag and other-musical-unresolved flag. Selection
can remain read-only while another musical card is suspended; mutation cannot.
clear_selection retains any failure card and changes its context serial.

## Explicit fields and permanent source

Legacy controls own placement tick, source in/out seconds and fade in/out seconds,
with Keep saved / Set value / Use default. Keep preserves exact Number spelling
and omission; Set parses bounded JSON Number text without f64 normalization;
Use default removes the optional field. Native controls own signed placement
frame, an atomic source-start/frame-count pair, and optional fades with lengths
and Linear/EqualPower curve. Required native settings permit only Keep/Set;
fade Keep preserves omitted/null/object, Remove restores omission, Set can
explicitly write zero fades. Do not convert coordinates or round saved values.

All editable raw text is bounded (legacy<=256 characters, native<=32). Preserve
invalid intermediate input as local draft and disable Save when invalid. Existing
number text is borrowed with Number::as_str(); oversized stored lexemes are
displayed in bounded form and retained with Keep. Choosing Set for such a value
starts blank and requires an explicit replacement; never submit a truncated
source or clone/serialize a full source payload in paint. Keep-only PreserveLoop
has no change. Authoritative combined window/fade/no-op validation remains on
the worker, without clamping or silently shortening fades.

Default to Keep current song extent. Extend song to include clip is explicit,
with initially unchecked schema-upgrade consent when schema 1 exceeds 128 bars.
No implicit loop shrink, retiming, schema/payload downgrade or source rewrite.
Display permanent original settings and current known source identity with
bounded known-field views, distinguishing missing/null/explicit values and
retaining opaque extensions without generic serialization. Original retained
does not assert that current settings differ. Drafts do not audition or record.

## One draft family and exact retry

At most one dirty editable family exists: session metadata, note or clip.
Main uses dirty_family() -> Result<Option<DraftFamily>,String>, plus
musical_dirty()/musical_unresolved() helpers. Multiple dirty families fail closed,
preserving every field; Save/Discard never pick one by branch order. A shared
retained-musical-action predicate applies in both request and dispatch.
Changing target/track/session, starting another family, history, archive,
attachment or Close resolves the active family with Save/Discard/Cancel. Save
continues only after acknowledgement; Discard affects that family only; Cancel
retains fields/selection and cancels the deferred action. Read-only Refresh and
retained-source Scan/Inspect remain exempt. Keep Close responsive during worker
reads without bypassing unsent drafts. Correct the weak light-theme unsaved-state
contrast as part of this UI packet.

Active completion context captures metadata, note and clip contexts together.
A stale successful completion preserves newer local fields, creates retained
intent evidence and never runs its deferred action. Exact retry retains the full
original operation/project/scope/patch/policy/revision while capturing a fresh UI
completion context for that transport attempt. Reconnect never retries it.

Only typed known uncommitted EditRejected/RevisionConflict permits explicit
Review latest / New choice / Release rejected. New choice starts from the reviewed
saved baseline and reapplies attempted non-Keep fields; Keep refers to the latest
saved value. First verify the retained draft's immutable source identity: legacy
asset/source-asset keys or native take/manifest/stream/sampleRate. A changed source
cannot inherit old trims/fades: keep the card and require Release rejected plus
fresh selection. The document shows saved source views, not authenticated full
descriptor/prefix bounds. It never silently resubmits. Missing/locked/wrong-kind targets can
release a reviewed known-rejected choice while retaining its original evidence.
Unknown outcomes can suspend their window-local card to inspect/navigate, but
cannot be replaced by a fresh musical mutation in either note or clip family.
This avoids a second unresolved musical edit. The exact card remains explicitly
retryable; metadata/navigation do not rewrite its durable request.
ReviewLatest/NewChoice resolve foreign dirty families without discarding the
retained failed values. Exact retries reject foreign dirty families and compare
both editor contexts before continuing any deferred action. Generic retry also
checks both contexts; a new draft cannot be captured and then overwritten by an
older metadata operation. Successful explicit ReleaseRejected may advance a still-exact metadata retry's editor contexts only when its document/revision, metadata fields and both pre-release editor contexts still match and no musical draft is dirty. Keep its durable command exact and clear its old continuation. NewChoice has no such exception. Audio start is blocked by unresolved musical cards and
multiple-family states; urgent Stop/Panic remains available.

Retained clip failures have their own visible buttons and evidence; generic
Retry routes by typed scope. Do not evict another unresolved intent or claim a
durable draft inbox. Closing an unresolved card warns that it loses window-local
fields and does not cancel a possibly committed edit. Audio/recovery guards,
urgent Stop and software-loopback-only input restrictions remain in force.

## Acceptance

Require owner and independent real-egui/injected-worker regressions for exact
selection/reordering/copies, raw numbers/default modes, integer/text boundaries,
legacy/native/event/locked views, permanent source display, all three draft
families and busy Close, stale completions, exact retry after review/navigation,
explicit known rejection/new choice/release and unknown-outcome retention.

After integrated fmt/test/Clippy/build, use fresh synthetic legacy/native source
clips only for actual UI edits, invalid fields/guards, trims/placement/fades,
Undo/Redo/Restore, committed source/render comparisons, complete archive/copy
and same-binary process exit/restart/reopen. Preserve all prior projects and
originals exactly. Record exact binary boundaries; existing backend, old UI and
CPU tests do not establish this new package's behavior. Waveform handles,
arrangement dragging, listening, physical capture/latency and Windows remain
separate unfinished requirements.
