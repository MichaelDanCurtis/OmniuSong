# OmniuSong implementation status

October 15, 2026, approximately 12:30 AM America/Los_Angeles.

## Open the current build

- Studio: [http://127.0.0.1:43178](http://127.0.0.1:43178)
- Project: `/Volumes/Main External/OmniuSong`
- Mac development app: `dist/OmniuSong.app`
- Launcher: `scripts/start-mac.command`

The local studio is running as a detached process. Its PID and log are in `.runtime/studio.pid` and `.runtime/studio.log`. The native app uses the same loopback service. No changes were made to Fuego.

## Implemented

- Versioned, validated note projects with integer musical timing, 16-bit velocities, expression lanes, track and lyric locks, revision conflicts, and accepted-candidate provenance.
- Local saves with atomic replacement and a previous-revision backup. Content-addressed audio assets and saved model selections.
- Developer plugin registry and durable generation queue with explicit audition/accept/discard, cancellation, restart interruption, and idempotent acceptance.
- Arrangement overview, piano roll, 16 pads, two-octave keyboard, track selection, mute, lock, explicit recording arm, local undo, tempo control, lyric editing, and a searchable model observatory.
- Original vocal capture flow and full-take preview. Capture is user initiated. Microphone operation has not been tested because the Mac is locked.
- Procedural arrangement sketches and an FM sample instrument, accurately labeled as procedural.
- An optional local Ollama lyric adapter. No Ollama service was present during the initial check; no lyric-model inference has been claimed.
- Actual local Basic Pitch transcription through ONNX, with an isolated Python environment, bounded worker output, cancellation signal, model checksum, and raw contour estimates retained alongside converted expression values.
- MIDI 1 type 1 export, MIDI 2 note/per-note-pitch UMP encoding, and timestamped UMP JSON export. The JSON format is explicitly identified as OmniuSong interchange, not a standard MIDI Clip File.
- Compiled Swift Mac shell with CoreMIDI UMP input. Native launch and physical MIDI performance remain unverified.
- A sourced catalog of 36 model, model-family, and supporting-tool candidates, plus product spec, roadmap, implementation plan, and hardware design.

## Verification

| Check | Result | Practical scope |
| --- | --- | --- |
| JavaScript syntax and catalog | Passed | 16 modules and 36 unique model entries |
| Automated tests | 21 passed, no skips | Project data, MIDI packet layout, export chunks, persistence conflicts, locks, queue review, cancellation, restart recovery, HTTP origin checks, and real model inference |
| Mac release build | Passed | Native executable and local app bundle compiled; not a launch or hardware test |
| Four-tone model inference | Passed | Real Basic Pitch ONNX checkpoint recovered MIDI pitches 60, 64, 67, 72 from a 3.2-second synthetic fixture; 231 contour points retained |
| Model checkpoint identity | Captured | SHA256 `2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec` |
| Browser session flow | Passed | Generate and audition a 92-note sketch, accept, add a note, save, reload, generate and accept a sample, play a sample key, transcribe the sample, accept notes |
| Saved browser state | Observed | Eight tracks, 95 notes, revision 4; all test material was created during development |
| Model observatory | Passed | Search and saved Keep selection through the UI |
| Responsive layout | Checked | 1440- and 680-pixel viewports showed no page horizontal overflow |
| Browser errors | None observed in tested flow | No captured error console entries |
| Physical sound and input | Pending | Listening quality, real microphone take, real keyboard, controller expression, latency and dropouts |

The initial cold model run took 10.755 seconds including imports and first-use preparation. A later automated fixture completed in approximately one second on this Mac. These are fixture observations, not a general performance benchmark. See `verification/basic-pitch.json`.

The FM test sample produced two estimated notes, which is a useful reminder that a successful integration is not a transcription-quality guarantee. Human singing, difficult timbres, dense polyphony, and expressive playback still need comparison and listening.

The initial SciPy 1.15.3 wheel failed to load a Fortran extension on this macOS version. Pinning SciPy 1.16.3 resolved it; setuptools 80.9.0 supplies the older dependency expected by pretty_midi. The resolved environment is recorded for reproducibility. [Upstream SciPy issue](https://github.com/scipy/scipy/issues/25635).

## Remaining work

The next useful slice is complete project archives with referenced audio and session switching. Current JSON export alone is not a complete audio-project backup. Add file import, missing-asset handling, and round-trip validation.

Then improve audio clip offsets and recording alignment, sustain and source/channel identities, note duration editing, per-note expression editing and playback, MIDI output, standard MIDI 2 Clip Files, and independent file validation. The preview stores expression that its current Web Audio synth does not yet perform.

Native realtime audio, Audio Unit/VST3/CLAP hosting, secure third-party plugin execution, large song-model workers, score-conditioned song editing, choir rendering, and Windows packaging remain roadmap work. No paid service, model gate acceptance, deployment, or personal-audio upload was performed.

The Mac was locked when native UI access was attempted. Continue software work without retrying the locked desktop. Native launch, microphone capture, and physical device checks can resume after unlock.

## Overnight continuation

The `OmniuSong overnight build` heartbeat is active every 30 minutes through 8:00 AM America/Los_Angeles on October 15. It resumes this chat and reads this file and IMPLEMENTATION-PLAN before choosing the next bounded slice. It should update this status after meaningful completed work and leave a morning review.

The continuation should preserve original performances, avoid paid or externally published operations, keep runtime environments isolated, and report actual inference separately from installation. Stop starting work at the morning cutoff.

## Studio preview

![OmniuSong studio with the accepted arrangement and Basic Pitch candidate](verification/studio-preview.png)
