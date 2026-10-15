# Audio clip placement, trim and fades

October 15, 2026. Implemented in the local development studio.

Select an audio track to open its clip controls. **Start** places the clip in the arrangement, using exact integer ticks from the song's beginning. One quarter-note beat is 960 ticks and one 4/4 bar is 3,840 ticks. **Source in** and **Source out** choose seconds within the full recording. The out point is the end of the selected window. **Fade in** and **Fade out** set linear fades in seconds within that window. Both default to zero; their combined length must fit inside the window. Save trim and fade changes together when resizing a clip.

Press Enter or choose **Save clip** to commit all pending fields together. Changing tracks or sessions and using transport, generation, Undo, or export controls also commits pending edits first. Invalid values stop that action and return the fields to the saved clip. Pending fields survive incidental studio redraws and trigger the browser's leave-page warning.

**Audition clip** plays the selected source window immediately in a separate preview, regardless of the track's mute switch. The main transport plays it at its saved start. Both respect source in/out, saved fades and the loop boundary. Fades use elapsed clip time, so a late scheduler resumes the envelope at the matching level. If the loop or decoded end shortens a clip, fade out ends at that audible boundary; when necessary, both fade lengths scale proportionally to fit. Saved settings remain unchanged. The arrangement shows the clip's selected length and position. Placement can extend the loop, up to 128 bars. Restore and Undo do not automatically shorten a loop.

## Keep the original

Trimming changes the arrangement's reference to the recording. It never rewrites the source file. Its checksum, byte length, MIME type, original duration, filename, origin, model information, and external metadata remain unchanged. The audio player and original download still contain the complete source, including the trimmed portions.

The project keeps these optional fields within `audio`:

| Field | Meaning |
| --- | --- |
| `startTick` | Position in the song; defaults to zero |
| `trimStartSeconds` | Source in; defaults to zero |
| `trimEndSeconds` | Source out; defaults to the full `durationSeconds` |
| `fadeInSeconds` | Linear fade from silence; defaults to zero |
| `fadeOutSeconds` | Linear fade to silence; defaults to zero |
| `editSource` | Version 1 snapshot of the original placement/trim/fade fields, preserving omitted defaults |

The first edit saves the original settings. Later edits keep the same snapshot. **Restore original clip** restores them even after reload or backup restoration. Track locks disable editing while allowing inspection and audition. Each committed edit adds a before/after journal entry. General Undo remains the existing 40-entry browser-tab stack; the journal is not yet a durable undo interface.

Windows must be finite, inside the full source, and at least one millisecond long. Fades affect arrangement playback and clip audition. Full-source players/downloads and source-window transcription do not apply them. Complete archives retain both settings and the full source. The existing WAV export renders procedural note tracks only; it is not an audio-clip mixdown. Crossfades, stretch, destructive render, waveform handles, and automatic recording-latency correction remain unimplemented. Physical listening and timing acceptance remain separate work.

## Review the source waveform

Choose **Show waveform** to decode the selected original locally. The overview uses real peak ranges from every decoded channel. Gray shows the full source, green highlights the chosen source window, and the gold line shows the linear clip-gain envelope. The source scale includes the full declared or decoded duration, whichever is longer; the status reports the actual decoded duration and format. A source window beyond the decoded end is explicitly reported as containing no decoded samples. Loop boundaries can shorten arrangement playback beyond what the source overview shows.

Valid pending fields update an **Unsaved window preview** immediately. The saved placement/length remains labeled **Saved clip** above it. Invalid drafts show the saved window with a warning. Previewing the waveform does not save an edit; Save clip, Enter, and existing transition rules still control commits. Locks allow waveform review while disabling editing. Hide waveform releases the view; Cancel waveform ignores a pending decode. Switching tracks or sessions also ignores a late result. Decode failures leave the original and saved project intact and can be retried by hiding/showing the view.

This is a peak overview, not a sample editor. It has no dragging, zoom, crossfades, pitch display, or destructive rendering. The overview accepts at most two minutes and 32 channels, with a total cap of 32 × 1,024 × 1,024 decoded samples (128 MiB of float samples) and 256 peak bins. The waveform cache retains up to eight small summaries; decoded audio shares the existing playback cache. Waveform data is regenerated from the archived original rather than stored in the project. No audio is uploaded.

## Transcribe the selected window

**Turn selected audio into notes** decodes the full source locally, extracts only the selected frames into a separate mono analysis WAV, and runs real local Basic Pitch inference. Estimated notes are placed at the clip's saved start. The original file remains referenced separately.

The transcription records the requested source window, actual decoded sample rate and frame bounds, and actual source seconds. Bounds round to the nearest decoded frame and cap at the decoded end. This matters when recording metadata duration differs slightly from the decoded container. Source windows, the canonical mono PCM WAV header, sample rate, frame count, and analysis duration are checked by the adapter. Mismatched or stale windows fail before inference. Raw model estimates remain relative to the analysis window, explicitly labeled `estimateTimeOrigin: analysis-window-start`.

Accepted notes are a separate track. Later moving or trimming the audio does not automatically move those notes. A candidate whose project changed remains subject to the existing revision checks. Full-source decoding is still required; this is not streaming transcription or a native sample-accurate recording claim.

## Verification

The **Clip editing qualification** session uses the procedural four-tone example and a locked synthetic note fixture. Three pending fields committed together when changing tracks: start tick 8,001, source in 0.8 seconds, and source out 2.4 seconds. The loop grew from one bar to three, while the complete player continued to report 3.2 seconds. Reload, invalid-window rejection, blocked invalid transitions, original restoration, Undo, locks, clip audition, and full-source keyboard playback were checked in Chrome. The full-source player did not toggle the main transport.

Actual Basic Pitch inference on the 1.6-second analysis WAV recovered pitches 64 and 67, starting at ticks 8,020 and 9,276. This browser decoded at 96 kHz, so the source window is frames 76,800–230,400 of 307,200. The original 44.1 kHz file remained byte-identical to the example. These are synthetic fixture observations, not general human-vocal transcription accuracy.

The downloaded 469,328-byte complete backup restored all four tracks, original clip settings, journal, and both original/analysis audio assets exactly into an empty temporary store. Independent Mido parsing confirmed three MIDI 1 note-ons including the two aligned estimates. The separate UMP JSON export remains OmniuSong interchange, not a standard MIDI 2 Clip File.

The full suite passes 84 tests with no skips; syntax/catalog checks cover 36 JavaScript modules and 36 catalog entries. Responsive checks at 1,440 and 680 pixels found no horizontal page overflow, and the tested browser flow had no console errors. A second real browser transcription passed the checked-WAV contract and was discarded without changing the session. Prior Magenta and precise-note qualification sessions remained unchanged.

Evidence: [receipt](verification/clip-editing.json), [source](verification/clip-editing-source.json), [saved project](verification/clip-editing-project.json), [MIDI 1](verification/clip-editing.mid), [UMP trace](verification/clip-editing.ump.json), and [studio screenshot](verification/clip-editing.png).

## Fade verification

The **Clip fades qualification** session retains the complete 3.2-second procedural source and a locked synthetic expressive note. Source in/out of 0.8–2.4 seconds and fades of 0.2/0.4 seconds committed together on a track change. Reload retained the exact values. Overlapping fades rejected the transition without changing the saved clip. Restore original returned to the full window with zero fades; Undo recovered the edits. All five fields disabled under a track lock, while audition remained available. The complete source player still reported 3.2 seconds. Clip audition and arrangement playback ran without browser console errors; this does not establish physical listening quality.

The actual 221,097-byte downloaded backup restored all three tracks, history, original snapshot, and the exact 282,284-byte source into an empty store and a new main-studio session. The original source SHA256 is `2a43aaacbb59bedceb5a15b4a99d5f70b880a8258cc788b09a8feddc12d74f55`. Pure-envelope and scheduler checks cover linear ramps, late starts, the loop boundary, proportional shortening, source-buffer preservation and unchanged analysis frames. The full suite passes 111 tests with no skips; syntax/catalog checks cover 40 modules and 36 entries.

Evidence: [fade receipt](verification/clip-fades.json), [source project](verification/clip-fades-source.json), [saved project](verification/clip-fades-project.json), and [studio review](verification/clip-fades.jpg).

## Waveform verification

The **Waveform review qualification** session uses the same procedural four-tone original and locked synthetic expressive note. The browser decoded the complete 3.2-second mono source at 96 kHz. Live fields previewed source 0.9–2.3 seconds and fades 0.25/0.35 seconds while the saved project remained at its prior revision. A reversed window fell back to the saved overview with an explicit warning, and Save rejected it. A valid edit saved and survived reload. Locked review, Hide/Show, Restore original and Undo updated the graph correctly. The existing Magenta studio session also showed its full eight-second, two-channel source without modifying its tracks or history.

The actual 221,100-byte browser backup restores all three tracks, history, trim/fade settings and the exact 282,284-byte original into an empty store and a new studio session. Ten earlier qualification sessions retain identical tracks and history.

The final suite passes 116 tests with no skips, including stereo transient preservation, final-bin coverage, decoded-source immutability, format bounds, shortened source envelopes, failed-load retry, an eight-summary cache, and stale decode isolation after track changes or cancellation. The HTTP test also checks the studio's complete transitive module graph. Syntax/catalog checks cover 42 modules and 36 model entries. Browser, backup and preservation receipts are in [waveform verification](verification/waveform.json), with a [saved project](verification/waveform-project.json) and [studio review](verification/waveform.jpg). Physical listening, recording alignment and waveform dragging remain separate work.
