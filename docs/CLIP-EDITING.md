# Audio clip placement and trim

October 15, 2026. Implemented in the local development studio.

Select an audio track to open its clip controls. **Start** places the clip in the arrangement, using exact integer ticks from the song's beginning. One quarter-note beat is 960 ticks and one 4/4 bar is 3,840 ticks. **Source in** and **Source out** choose seconds within the full recording. The out point is the end of the selected window.

Press Enter or choose **Save clip** to commit all pending fields together. Changing tracks or sessions and using transport, generation, Undo, or export controls also commits pending edits first. Invalid values stop that action and return the fields to the saved clip. Pending fields survive incidental studio redraws and trigger the browser's leave-page warning.

**Audition clip** plays the selected source window immediately in a separate preview, regardless of the track's mute switch. The main transport plays it at its saved start. Both respect source in/out and the loop boundary. The arrangement shows the clip's selected length and position. Placement can extend the loop, up to 128 bars. Restore and Undo do not automatically shorten a loop.

## Keep the original

Trimming changes the arrangement's reference to the recording. It never rewrites the source file. Its checksum, byte length, MIME type, original duration, filename, origin, model information, and external metadata remain unchanged. The audio player and original download still contain the complete source, including the trimmed portions.

The project keeps these optional fields within `audio`:

| Field | Meaning |
| --- | --- |
| `startTick` | Position in the song; defaults to zero |
| `trimStartSeconds` | Source in; defaults to zero |
| `trimEndSeconds` | Source out; defaults to the full `durationSeconds` |
| `editSource` | Version 1 snapshot of the original placement/trim fields, preserving omitted defaults |

The first edit saves the original settings. Later edits keep the same snapshot. **Restore original clip** restores them even after reload or backup restoration. Track locks disable editing while allowing inspection and audition. Each committed edit adds a before/after journal entry. General Undo remains the existing 40-entry browser-tab stack; the journal is not yet a durable undo interface.

Windows must be finite, inside the full source, and at least one millisecond long. The preview adds no fades, crossfades, stretch, destructive render, waveform handles, or automatic recording-latency correction. Physical listening and timing acceptance remain separate work.

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
