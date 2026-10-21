# Native clip edit backend contract

Status: backend implemented and independently software-qualified. See the
[425-entry gate](../verification/rust-native-clip-editing-tests.json) and
[independent review](../reviews/RUST-CLIP-EDITING.md). This contract does not qualify
native clip controls, a waveform, input recording, listening, or a new packaged
binary. The later [216d native clip walkthrough](../verification/rust-native-clip-editor-ui.json) qualifies its controls separately; earlier 425/85ef receipts retain their own boundaries.

## Scope and identity

Implement finite UpdateClip and RestoreClip operations through the existing
replay-first ProjectEditIntent transaction, durable history, receipt, session
service and complete archive/copy paths. No Add/Delete/source replacement,
resampling, quantization, event reinterpretation or device access in this packet.

DocumentClipAddress is a strict tagged enum: LegacyAudio { track_id } is one
implicit clip on an audio track; NativeAudio { track_id, clip_id } selects an
audio-frame TakeClip on a recording track. Equality lookup preserves legal legacy
empty/Unicode/control keys (200 scalars/800 bytes); native keys keep their existing
nonempty 96-byte ASCII rules. Never target array indices. Require locked == false
and the declared track kind. Project identity stays in the enclosing intent.

ClipPatch has matching LegacyAudio and NativeAudio variants, with strict typed
fields. Every patch field is explicit on the wire. FieldChange<T> is Keep,
Set { value }, or Remove. ValueChange<T> is Keep or Set { value }. Unknown fields,
mismatched representations and arbitrary merge patches reject.

LegacyAudioClipPatch owns start_tick (FieldChange<u64>) and the four trim/fade
seconds (FieldChange<serde_json::Number>). Keep retains exact current field
presence and JSON number representation; Remove restores omission; Set writes
the exact requested typed number. NativeAudioClipPatch owns position_frame
(ValueChange<i64>), source_window (ValueChange<AudioSourceWindow>, with
start_frame/frame_count), and fades (FieldChange<AudioFades>). Native source
TakeId/manifest/stream, clip ID, range unit and position.sampleRate are immutable.
Raw event ranges are not audio duration and are rejected by these operations.

The full immutable document is cloned and validated before a candidate is
returned. Original documents stay unchanged on failure. Exact JSON settings
presence/value equality defines a change: Set an omitted default or Remove an
explicit default is an intentional edit. Set an identical value or Remove an
already absent field is not. Full-document no-ops reject. Extent-only changes
are allowed without creating an original snapshot or promoting a payload.

## Originals and format evolution

Legacy audio.editSource version 1 is already owned by the schema. On first
actual setting change, snapshot only originally present startTick,
trimStartSeconds, trimEndSeconds, fadeInSeconds and fadeOutSeconds as raw Values
under { version:1, original:{…} }. Restore changes those five field presences and
values only. Retain the complete immutable snapshot, including outer unfamiliar
metadata, across every further edit and Restore. Never rewrite duration, asset,
origin, byte length, recordingTiming, notes, packets, clock observations or
unrelated metadata.

Introduce recording payload version 2 explicitly. Version 1 remains readable
and forbids clip.editSource, even null. Version 2 accepts the new optional
owned editSource slot; clips without it remain ordinary originals. New recording
publication stays version 1 and must reject caller-supplied original snapshots.
Historical canonical publication requests remain byte-identical when the new
optional field is absent.

On first actual native setting edit, promote only the selected recording
payload to version 2 and capture clip.editSource as
{ version:1, original:{ range, position, [fades] } }. Original range and position
are required; fades retains its exact absent/null/object representation. Only
these three keys are allowed inside original; outer snapshot extensions remain
opaque and exact. Validate snapshot/current audio units and identical sample
rate, with no nested snapshot. Update the selected raw Value surgically; do not
deserialize/reserialize sibling clips or the whole payload. Restore retains
version 2 and the whole immutable snapshot, restores the exact three original
settings, and never downgrades the format.

TakeClip gains an optional edit_source: Option<Value> projection, omitted when
absent, plus original_view() -> Result<Option<TakeClip>> for semantic resolver
validation. Project validation additionally checks raw editSource presence so
explicit null cannot bypass version-1 rules or snapshot validation. This typed
projection is a view, not a serializer for replacing source documents.

## Bounds and arrangement extent

Legacy settings retain existing validation: finite seconds, declared source
duration, startTick <= 1e9, at least .001 seconds (existing 1e-9 tolerance),
nonnegative fades fitting the window. Do not decode codecs during a semantic
edit. Existing asset integrity still applies; actual codec/PCM duration/rate
support remains a renderer check. A valid saved view can exceed the renderer's
120-second/64-MiB limits without losing its source.

Native source ranges are checked positive half-open frame intervals within
i64::MAX; signed position + count must fit i64. Fade frame sums must fit the
window. Store resolution verifies BOTH current and retained-original ranges
against the authenticated selected stream's initial source position and sealed
verified prefix end, including units and exact PCM sample rate. Gaps and damaged
tails remain retained evidence; selected gaps still reject rendering.

ClipTimelinePolicy is explicit PreserveLoop or ExtendLoop {
allow_native_schema_upgrade }. PreserveLoop changes no bars/schema. ExtendLoop
never shrinks. Legacy uses this pinned f64 evaluation order:
endTicks = startTick + (trimEndSeconds - trimStartSeconds) * tempo * 960 / 60;
requiredBars = ceil(endTicks / 3840). Validate all intermediates finite,
nonnegative and <= MAX_NATIVE_BARS before narrowing. This is a declared-seconds
estimate, not a decimal/sample-exact conversion; saved numbers are unchanged.

Native extent uses the exact validated tempo f64's binary ratio. Let
e = encodedExponent - 1023 (4..8 for tempo 30..300),
m = 2^52 + fraction, d = 2^(52-e), and
E = max(checked(position.frame + count),0). Then
requiredBars = ceil(E*m / (sampleRate*240*d)), with checked u128 arithmetic,
exact integer ceiling and native maximum checks before narrowing. Zero wholly
preroll extent needs no bars. Promote schema 1 to 2 only if requested extent
exceeds 128 bars and explicit permission is true. Do not implicitly shrink or
downgrade during Restore/Undo/Redo.

## Persistence, archives and verification

Pure, pre-write semantic edit failures map to EditRejected. In the native clip UI packet, clip operations also authenticate the current document references and owned files after replay/revision checks and before mutation. An explicitly Set native audio source window outside that same authenticated descriptor/sealed-prefix interval is a known EditRejected choice. This narrow comparison does not apply to Keep, Restore, identity/rate/unit damage or any source-read error. Source damage,
unknown I/O/SQL failures and post-commit uncertainty keep conservative handling.
Replay checks the identical original request before revision admission, verifies
sources and returns its original result without applying again or rebasing.

Archives explicitly admit the two operations and validate bounded postimages:
exact target/type/lock, each requested Set/Remove, required native invariants,
Restore equality to retained original settings/presence, and requested loop
coverage. Keep cannot prove its preimage from a receipt alone. Neither snapshot
originality nor immutable historical before-state can be inferred from a lone
postimage; do not claim otherwise. Restore-as-copy remaps project/operation
identity only, retaining clip IDs, sources and opaque strings exactly.

Acceptance requires adversarial pure DTO/core tests; real transactional
retry/conflict/rollback/history/reopen/archive/copy tests with authenticated
synthetic sources; forged-rehashed postimage rejection; exact untouched metadata
and original source byte comparisons; and production prepare + RenderCursor PCM
comparisons for placement/windows/fades/Restore. Preserve current native one-shot
full-content rendering independent of bars, and the current source-relative
fade law. No UI or device acceptance is implied by this backend packet.
