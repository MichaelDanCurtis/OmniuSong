# Editable performance projection

This contract covers the read-only derivation of editable notes from authenticated saved local keyboard/pad performances. It is a necessary stage of the native recording workflow. Atomic publication, project ownership, archive integration and native controls require their own implementation and verification before this becomes a musician-facing feature.

The original six-stream take remains authoritative. A note projection is a procedural interpretation of retained renderer events, not AI inference, audio transcription, MIDI transport or calibrated controller timing. MIDI 2.0 and MPE remain mandatory subsequent capabilities.

## Dependency and authority boundary

`omniusong-performance` is a portable Rust library beneath the engine and future store integration. It depends on core musical contracts and recording readers, with no device, application, store or engine dependency. Shared owned event/packet types, the original performance codec and the contextual checkpoint verifier live here. The engine preserves its existing public paths through reexports.

Queue/tap ownership, realtime callbacks, native devices, writer lifecycle, normal sealing and its private native permit remain in the engine. Extracting a verifier must not grant a new way to mint normal recording completion. Existing recording namespaces, serialized bytes, descriptor identities and checkpoint hashes remain unchanged.

The projection entrypoint requires a held `SealedJournal` with an existing cumulative `InspectionBudget`. It authenticates the complete sealed source and its owned performance profile, then checks the request's exact take, manifest, descriptor, original-row and physical-file identities, selected six-stream prefix and checkpoint hash. Derivation and output construction use that same sticky governor. An independently supplied inspection or hash is not authority to skip verification.

All reads, allocation/copy/decode work and output are bounded. Cancellation or resource exhaustion returns no partially usable projection. The implementation must not raise the current recording ceilings to make a test pass. Source limits are distinct from the derived-note and metadata limits; an oversized interpretation must refuse visibly instead of silently shortening the original.

The bounded sealed reader anchors the opening ordinary file's metadata and checks SQLite's actual main-file `HAS_MOVED` result. Recheck before/after physical inspection, chunk reads and complete visitation, and after interpretation/digest construction. Path replacement, same-inode edits and new SQLite sidecars must refuse; a new pathname hash cannot be combined with an older logical snapshot. This adds no auxiliary file descriptor that could be mistaken for a SQLite exclusion lock. An unsupported VFS identity operation refuses explicitly; actual Windows identity support remains an independent acceptance item. Legacy unbounded snapshot/export behavior retains its existing scope.

## Explicit interpretation request

Version 1 selects a half-open output-frame region within the authenticated checkpoint and explicit, sorted, unique local source handles. It pins the exact binary tempo in 30–300 BPM, a nonnegative destination tick, 960 PPQ, nearest-ties-up endpoint rounding and release-gate duration semantics. It carries separate reject or omit-with-disclosure choices for collapsed intervals, incomplete endings and region-crossing notes. Endpoints outside the musical document's start/duration bounds refuse the whole request under either policy.

Convert frame deltas using checked integer arithmetic over the exact binary tempo ratio. Do not multiply large frame counts through `f64`, reinterpret GUI timestamps as audio frames, independently quantize durations or pad a collapsed duration to one tick. Keep exact source onset/end frames and tempo bits so rounding residuals remain reconstructible. The resulting `NoteValues` must satisfy the existing document bounds, including exact 16-bit velocity zero.

A bounded selection may exclude sources or crossing/incomplete observations only under its explicit policy. The review retains omissions, reasons and counts. Refusal is preferable to a partial result presented as a complete performance. No quantization grid, resampling, inferred alignment, release tail or extra recorded audio follows from conversion.

The 20,000-observation detail cap applies to the chosen interpretation, not every note in the original. Validate all source events with at most 64 active voice slots. Excluded/outside observations contribute exact source-wide counts and a domain-separated streaming digest; they do not occupy detailed output rows. Pre-region active voices may retain provisional details until their intersection is known. Selected/intersecting omissions retain individual reasons. Never cap the source to make a small selected region fit.

## Event and voice semantics

The existing contextual verifier checks that AppliedEvent witnesses in GUI streams match complete Applied batches in the control stream. Derive each event once from those authenticated batches. Retain malformed discarded/rejected originals without interpreting their payloads or indexing them as valid source events.

Voice identity includes take, worker, session, Panic generation, source handle, connection generation, original note ordinal and voice ordinal. Repeated pitch is not identity. Different sources may use the same voice ordinal, and Panic can restart source ordinals. Keep those identities separate and retain exact source record positions, packet/event order and original observed ticks.

The projected duration ends at the first actual release trigger: an unsustained NoteOff, the appropriate source's sustain-up, observed automatic release, an explicit engine clear or a Finish cut. Preserve the first physical key-up and release velocity separately from gate release and later `VoiceEnded`. A repeated or late NoteOff must not overwrite the first observation. A NoteOn with zero velocity still creates its own note.

Pulse and drum automatic release require retained transition evidence. Do not infer an observed transition merely from a scheduled duration. An earlier explicit release can supersede that automatic release. Sustain belongs to its exact source; another source's pedal does not release the note. Engine cuts and selected unknown boundaries must retain their actual reason and must never be fabricated as human NoteOff events.

Validate lifecycle and identity consistency as well as record shape. A coherently rehashed source containing an impossible transition, missing/duplicate/future voice or contradictory counters must not become an authenticated musical interpretation merely because its generic hashes match.

## Review and subsequent publication

A versioned review binds the exact request, ordered notes, original observations, omissions and disclosures through a domain-separated canonical digest. Derived note IDs are deterministic from complete original identity, independent of array order, pitch and destination project. Notes retain their patch/Pulse provenance and exact frame timing; the existing saved-note synthesizer is a different interpretation from immutable original PCM.

The future native review must disclose rounding, omitted or cut notes, unknown tail, sustain/envelope/patch differences and uncalibrated GUI observations before separate confirmation. No default simultaneous original-PCM and derived-note playback may double the performance.

Publication must be one typed reversible operation that commits project, history, source ownership and receipt atomically, with replay-first exact request matching. A sequence of AddNote calls or a caller-supplied digest is insufficient. Use the shared verifier during publication; keep a typed owned take reference reachable even when the original audible clip is later removed. A note's opaque `takeId` extension does not establish archive ownership.

The integrated format must explicitly capability-gate the new owned projection semantics; it cannot reinterpret unknown fields on existing schema-1/2 note tracks. The planned schema-3 boundary, archive closure/remapping, source-semantic revalidation and note editing/rendering integration remain unfinished until separately implemented. Preserve old schemas and historical opaque data exactly.

## Qualification

Use actual device-free renderer → writer → authenticated recovered sealed v1/v2 fixtures, retaining Finish where observed. These fixtures do not grant native normal-seal authority. Verify overlap, source/Panic identity, sustain, Pulse/drums, early release, exact attack/release velocities, cuts, unknown/crossing intervals, endpoint rounding/collapse and deterministic repeat.

Independently rehash semantically false records to test the interpreter's own refusal. Check sticky cancellation and resource limits across authentication, decoding and output. Compare complete source bytes and rows before and after held readers close. Preserve the regular studio and installed app; software kernel qualification alone establishes no native UI, listening, capture, thirty-minute capacity, archive publication or Windows acceptance.
