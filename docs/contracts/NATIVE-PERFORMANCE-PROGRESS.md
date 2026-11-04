# Native performance progress and owner transitions

Progress is retained evidence from actual admission/callback observations, not authorization to resume a stream. The parent continues to reject missing scopes, regressed frames or sequence watermarks, changed immutable readiness and changed first faults.

## Ordinary playback after a recorded performance

The worker keeps the last announced same-worker performance progress in its existing `last_live` slot before ordinary playback replaces the quiescent performance owner. Status and Stop retain that ended scope. An unannounced preparation cannot replace it. The existing authenticated new LiveReady or first real performance Ready remains the boundary that clears old progress. The parent disappearance validator is unchanged.

The `4981d54a…` native walkthrough reproduced the former disappearing-summary failure after a successful Finish/attachment; that failed run remains recorded separately from a passed playback after restart.

## Panic across callback acknowledgment

The callback may clear its new-generation applied counter before the off-callback admission owner adopts the acknowledgment. While Panic is pending, public progress still carries the old admission scope. Publishing the callback's cleared counter with that old scope would falsely regress an already reported applied watermark.

On the first successful `begin_panic`, admission samples and privately pins the actual old-generation applied watermark **before** publishing the requested generation. Same-Panic retries retain that pin. Pending progress retains its old scope and accepted watermark, has no lease credit, and uses the pinned applied watermark only if the effective published scope still equals the admission scope. It is a conservative last observation; it does not invent application of queued batches.

`finish_panic` alone adopts the next scope after actual callback acknowledgment, clears the accepted/pinned watermarks, and rotates lease credit. First-fault scope and cause remain authoritative: a different fault scope never receives the old pinned watermark. No arbitrary maximum clamp or inferred generation is used. The callback and its recorded new-generation TapEnd continue to report their actual cleared applied counter.

The `a948b887…` native attempt failed with the earlier generic progress-regression message. Its retained checkpoint proves 565248 frames and 1104 callbacks with an unknown tail, without normal sealing or attachment. Focus cleanup can request Panic, and the race above is independently reproducible, but the retained prefix does not establish which comparison failed in that attempt. New bounded fixed-field diagnostics preserve the failing old/new revision, scope, state, frames, accepted/applied watermarks and first fault for future diagnosis.

## Acceptance boundaries

Use actual CPU recording pairs for clear-before-adoption, repeated pending observations, four-callback queued discard, original TapEnd consistency and first-fault scope tests. Test PerformanceSession snapshots under its background status lock and parent rejection of genuine regressions. Independently verify zero callback allocations/deallocations. Source tests do not qualify the corrected native GUI: repeat Record/Ready/keys/pads/Finish/seal/explicit attachment and same-generation Play/Status/Stop on the corrected package. Preserve unsuccessful stages and prior receipts. No microphone or input stream is required.
