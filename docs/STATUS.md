# OmniuSong implementation status

October 15, 2026, approximately 4:45 AM America/Los_Angeles.

## Open the current build

- Studio: [http://127.0.0.1:43178](http://127.0.0.1:43178)
- Project: `/Volumes/Main External/OmniuSong`
- Mac development app: `dist/OmniuSong.app`
- Launcher: `scripts/start-mac.command`

The local studio is running as a detached process. Its PID and log are in `.runtime/studio.pid` and `.runtime/studio.log`. The native app uses the same loopback service. No changes were made to Fuego.

## Implemented

- Versioned, validated note projects with integer musical timing, 16-bit velocities, expression lanes, track and lyric locks, revision conflicts, and accepted-candidate provenance.
- Local saves with atomic replacement and a previous-revision backup. Content-addressed audio assets and saved model selections.
- Complete `.omniusong` backups include referenced audio, source recordings, expression, and accepted-candidate history. Restore validates the whole bundle and audio checksums before publishing a new session; existing projects remain intact.
- A saved-session picker flushes pending edits before switching and remembers the active session after reload. Audio asset writes now use atomic replacement, and reads detect damaged content.
- Developer plugin registry and durable generation queue with explicit audition/accept/discard, cancellation, restart interruption, and idempotent acceptance.
- Arrangement overview, piano roll, 16 pads, two-octave keyboard, track selection, mute, lock, explicit recording arm, local undo, tempo control, lyric editing, and a searchable model observatory.
- Source-aware note input with distinct UUID voice lifetimes, channel/group separation, repeated-note pairing, sustain, physical key-release timing, release velocity, and safe cancellation during audio startup. Focus loss, panic, and device removal finalize held notes. A visible Sustain button supports the computer and on-screen keyboard.
- Bounded original MIDI 1/UMP capture while an armed selected track plays. Source IDs and receipt times are retained; the Mac bridge also carries native host-tick strings without rounding them through JavaScript numbers. This is wired and software tested; native endpoint delivery remains unverified.
- Recording now has explicit permission, recording, finalization, saving, and recovery states. Session changes remain blocked through pending permission and storage; cancellation stops a late microphone stream. Failed saves retain the original in the tab for retry/download, and retries do not duplicate the track. Mocked recorder tests cover denial, disconnect, final data events, timers, and cleanup. Physical microphone operation remains untested because the Mac is locked.
- Local audio import preserves original bytes and metadata, with a full-take player and original download. Imports are bounded to two minutes and 30 MiB and must decode in the browser. WAV is verified end to end; MP3/M4A/FLAC/Ogg/WebM depend on browser decoding. MP3 and FLAC byte delivery are covered by service tests.
- A clearly labeled synthesized four-tone example exercises import and actual Basic Pitch transcription. Imported and unquantized note starts now appear in the piano roll without changing their saved timing. Exact pitch, start, duration, and 16-bit velocity controls are now implemented. Repeated notes in one cell are separately selectable; locked notes can be inspected. Original musical values survive save/reload and backups through a validated snapshot, with Restore original and tab-local Undo. Shortening past existing expression points is rejected; raw input and model metadata remain unchanged. See NOTE-EDITING.
- Audio delivery now supplies byte lengths and seek ranges, so the tested original player reports its 3.2-second duration and supports seeking.
- Procedural arrangement sketches and an FM sample instrument, accurately labeled as procedural.
- An optional local Ollama lyric adapter. No Ollama service was present during the initial check; no lyric-model inference has been claimed.
- Actual local Basic Pitch transcription through ONNX, with an isolated Python environment, bounded worker output, cancellation signal, model checksum, and raw contour estimates retained alongside converted expression values.
- Actual Aria piano continuation on MLX in an isolated environment, with pinned official code and a 2.63 GB checkpoint verified on each run. A compatibility subclass supplies the upstream sampling path's missing cache boundary. The worker validates and bounds input, isolates new notes from the tokenized source, retains model timing, supports cancellation, and records runtime identity. Studio continuation now uses the durable candidate queue with source-bar selection, a bounded response token budget, seed, immediate answer audition, acceptance, and discard. The source is projected as piano with boundary cropping and no expression; the stored source is untouched.
- A new **Aria studio qualification** session verifies two different seeded 21-note answers: one discarded, one accepted. Acceptance expands its loop from two bars to five. Model identity, runtime, source scope, original model timing, and generation evidence are retained in accepted-track metadata and complete backups.
- Four-bar editor pages cover the full 128-bar session limit, with pitch-range choices covering MIDI notes 0–127. The bar-five note editor, high-note edit, undo, save, and reload were exercised in the browser. Startup controls remain inert until session initialization completes. Queued jobs now reject a changed source revision before running inference.
- Actual Magenta RealTime 2 Small audio generation through the pinned official exported MLX graph and an isolated runtime. Six selected model assets total 1.04 GB and are verified on each run. The bounded worker supports text style and optional pitch/onset/sustain guidance; it preserves source data and independently verifies the generated WAV. Studio controls now offer style prompts, optional selected-note guidance, 2/4/8-second durations, start-bar placement, immediate audition, acceptance, and discard. This is short offline generation; live accompaniment remains unqualified.
- The **Magenta studio qualification** session verifies real candidate generation from a locked source, audition/discard without project changes, and acceptance of an eight-second clip at bar two. Acceptance grows the loop from two bars to five. Exact projected guide notes, source revision, model/export-graph identity, runtime evidence, and audio bytes remain available in accepted-track metadata and complete backups.
- Audio clip controls now edit exact placement and source in/out points, with atomic pending-field saves, locks, audition, original-settings restoration after reload, and tab-local Undo. Full source bytes and metadata remain intact. Arrangement playback honors the selected window and loop boundary; late scheduling adds elapsed time to the source in point. Basic Pitch transcribes only the selected analysis frames and aligns its estimates to the clip, retaining an explicit source-window receipt. See CLIP-EDITING.
- A separate **Magenta Small · local audio preview** session retains a locked procedural note guide plus style-only and note-guided four-second generated audio. Both raw players, transport start/stop, generated-audio labels, and complete archive restoration are verified. Keyboard shortcuts leave native media controls and focused buttons' Space behavior intact.
- A separate **Aria piano · local model preview** session contains 12 locked procedural prompt notes followed by 21 AI-generated notes across five bars. MIDI, WAV, project, UMP JSON, and a reproducible receipt are saved in `docs/verification/`. The audio uses the existing preview synth, not a neural piano renderer.
- MIDI 1 type 1 export, MIDI 2 note/per-note-pitch UMP encoding, and timestamped UMP JSON export. The JSON format is explicitly identified as OmniuSong interchange, not a standard MIDI Clip File.
- Compiled Swift Mac shell with CoreMIDI UMP input. Native launch and physical MIDI performance remain unverified.
- A sourced catalog of 36 model, model-family, and supporting-tool candidates, plus product spec, roadmap, implementation plan, and hardware design.

## Verification

| Check | Result | Practical scope |
| --- | --- | --- |
| JavaScript syntax and catalog | Passed | 36 modules and 36 unique model entries |
| Automated tests | 84 passed, no skips | Project, MIDI, persistence, queue, real model inference, and archive checks, plus source/channel/group isolation, repeated and sustained notes, pending audio cancellation, focus/device loss, control messages, bounded capture, zero-velocity MIDI 2 notes, release velocity, and raw-input backup round trips; recording lifecycle, cancellation, retained-original retries, audio byte delivery, seek-range checks, actual Aria inference, bounded invalid requests, cancellation before launch, phrase boundaries and budgets, exact answer placement, loop expansion, complete continuation archives, immediate audition, timeline bounds, queued-source revision conflicts, Magenta 25 Hz note projection, real generated PCM audio, generated-origin archive round trips, worker startup cancellation, scoped audio candidates, fractional-tempo boundaries, clip placement, loop-boundary/late scheduling, and placed-audio transcription; precise note edits, expression-tail safeguards, original snapshot validation, journaled deletion, edited-note archive round trips, source-preserving clip edits/restoration, original clip snapshot validation, source-window frame checks, mismatched prepared-WAV rejection, exact analysis WAV cropping, trimmed and late playback scheduling, and actual inference on a cropped source |
| Mac release build | Passed | Native executable and local app bundle compiled; not a launch or hardware test |
| Four-tone model inference | Passed | Real Basic Pitch ONNX checkpoint recovered MIDI pitches 60, 64, 67, 72 from a 3.2-second synthetic fixture; 231 contour points retained |
| Aria local inference | Passed | 38 prompt tokens, 64 new tokens, and 21 new complete notes. Latest warm-filesystem receipt: 2.500 seconds total, 0.560 seconds generation, 2,719,036,048 bytes MLX peak allocation. One fixture, not a live-performance benchmark |
| Aria studio workflow | Passed in browser | Two real seeded continuations differed; discard left the two-bar source unchanged; audition ran before acceptance; acceptance produced five bars with 33 notes. Bar-five editing at pitch 127 saved and undid correctly, reload retained the accepted notes, and a complete archive restored with identical tracks. Live receipt: `verification/aria-studio.json` |
| Magenta Small local audio | Passed | Two actual four-second 48 kHz stereo renders, with and without the four-note guide, have different checksums. Generation phases 1.127 / 1.131 s; total workers 3.051 / 2.999 s; peak MLX allocator approximately 534 MB. No clipped samples in these fixtures. Full receipt and limitations: `verification/magenta.json`, MAGENTA-QUALIFICATION |
| Magenta candidate and transcription chain | Passed in browser | Generated 2/4/8-second candidates, used note guidance and style alone, auditioned/discarded without edits, accepted eight seconds at bar two, then transcribed that clip into 23 aligned estimates. Reload retained five bars, 35 total notes, the clip position, and the source. A 2,456,521-byte complete archive restored all tracks and both audio assets identically. MIDI 1 independently parsed with 23 estimated note-ons starting at tick 3,956; own UMP JSON has 954 packets. Receipt: `verification/magenta-studio-flow.json` |
| Magenta studio preview | Passed in browser | Exact source guide retained; two generated WAVs labeled separately from human takes; both players report four seconds; transport and focused raw-player keyboard controls work. A 1,346,668-byte archive restored three tracks and both audio assets identically |
| Aria output and preview | Passed | Original request preserved; separate source and answer tracks; actual second inference in tests; MIDI 1 independently parsed by Mido with 33 note-ons. Studio session selection, prompt lock, transport start/stop, and model qualification label checked in browser |
| Model checkpoint identity | Captured | SHA256 `2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec` |
| Browser session flow | Passed | Generate and audition a 92-note sketch, accept, add a note, save, reload, generate and accept a sample, play a sample key, transcribe the sample, accept notes |
| Saved browser state | Observed | Original session: eight tracks, 95 notes, revision 4. Restored copy: eight tracks, 96 notes, revision 1 after a note edit and session-switch test. All material was created during development |
| Complete backup | Passed | Downloaded 446,150-byte archive restored through the live service; all eight tracks, 95 notes, provenance, and two audio assets matched the source, including byte-for-byte media comparison |
| Input browser flow | Passed | Three distinct notes, including two presses of C4, recorded under sustain. Pedal release cleared them; physical key durations and longer sustained durations survived save/reload. Panic cleared active input; keys played while stopped added no notes. The separate Input qualification session contains this test material |
| Audio import and transcription | Passed in browser | Imported the procedural 3.2-second WAV, ran real local Basic Pitch, auditioned and accepted pitches 60, 64, 67, 72, then reloaded. Complete archive restored into an empty temporary store with all tracks/history and both original/analysis audio byte-identical |
| Failed audio save recovery | Passed in browser | A separate temporary service deliberately failed its first write. UI blocked session changes, exposed retry/original download, downloaded exact original bytes, and retried into one audio track. The temporary service was stopped afterward |
| Session switching | Passed in browser | A pending note edit saved before switching; original retained its state, restored copy retained the edit and reopened after reload |
| Browser backup file selection | Permission limitation | Actual backup download verified; automated restore-file selection requires Chrome extension file URL permission. The restore service and complete bundle round trip are verified separately |
| Model observatory | Passed | Search and saved Keep selection through the UI |
| Responsive layout | Checked | 1440- and 680-pixel viewports showed no page horizontal overflow |
| Browser errors | None observed in tested flow | No captured error console entries |
| Precise note editing | Passed in browser | Two repeated notes independently selectable; pending fields commit atomically on track changes; invalid shortening blocks the transition; exact pitch/start/length/velocity survive reload; persistent Restore original, deletion/Undo, locked inspection, and actual Basic Pitch estimate edit/restore verified. Downloaded 2,455,739-byte backup restored all tracks, snapshots, history, raw packets and two assets exactly. Independent Mido parse confirms 26 note-ons and exact edited timing; UMP trace retains full velocity and moved expression. See NOTE-EDITING and verification/note-editing.json |
| Audio clip editing | Passed in browser | Start tick 8,001 and source 0.8–2.4 s save together and survive reload; full source remains 3.2 s. Restore/Undo, invalid-transition guard, locked inspection/audition, preview and full-source keyboard controls verified. Real Basic Pitch returns pitches 64/67 at ticks 8,020/9,276. Downloaded 469,328-byte archive restores four tracks, original settings, history and two assets exactly; independent MIDI 1 parse confirms aligned notes. See CLIP-EDITING and verification/clip-editing.json |
| Physical sound and input | Pending | Listening quality, real microphone take, real keyboard, controller expression, latency and dropouts |

The initial cold model run took 10.755 seconds including imports and first-use preparation. A later automated fixture completed in approximately one second on this Mac. These are fixture observations, not a general performance benchmark. See `verification/basic-pitch.json`.

The FM test sample produced two estimated notes, which is a useful reminder that a successful integration is not a transcription-quality guarantee. Human singing, difficult timbres, dense polyphony, and expressive playback still need comparison and listening.

The initial SciPy 1.15.3 wheel failed to load a Fortran extension on this macOS version. Pinning SciPy 1.16.3 resolved it; setuptools 80.9.0 supplies the older dependency expected by pretty_midi. The resolved environment is recorded for reproducibility. [Upstream SciPy issue](https://github.com/scipy/scipy/issues/25635).

## Remaining work

Aria continuation, Magenta audio candidates, full timeline review, and precise note editing are implemented and browser verified. Clip reposition/trim controls and source-window transcription are now implemented and verified. Next add a metronome/count-in and recording alignment controls. Keep sustained streaming and device-latency acceptance separate. Reliable session switching, complete backups, source-aware input, recording-state safeguards, and local audio import are implemented. Recording alignment, expression editing/playback, and durable general undo remain follow-on work. Archive limits are in PROJECT-ARCHIVE; input semantics are in HARDWARE-MIDI; recording behavior is in RECORDING; precise note editing and original snapshots are in NOTE-EDITING; audio placement, trims and source-frame receipts are in CLIP-EDITING.

Raw input is limited to 4,096 messages per project, with a visible warning and omission count when full. Recorded timing uses browser receipt and elapsed time; native host ticks are retained but not calibrated to the audio clock. Same-pitch MIDI note-offs pair FIFO within one source/group/channel. Original packets and generated edited-note exports remain distinct. Software source-loss tests do not establish a native or physical controller round trip. See `verification/input-session.json` for the browser save receipt.

Then improve recording alignment, clip fades/waveform handles, per-note expression editing and playback, MIDI output, standard MIDI 2 Clip Files, and independent file validation. The preview stores expression that its current Web Audio synth does not yet perform.

Aria's synthetic-source studio workflow does not qualify live takeover, simultaneous recording, hardware latency, listening quality, or general continuation quality on human performances. Response tokens are a compute/length budget, not an exact number of bars. Prompts over 15 seconds, 256 notes, or 1,024 tokens are rejected. Acceptance appends after the selected phrase and can overlap other existing tracks; it does not rearrange the song. Code/model identities and the compatibility fix are documented in ARIA-QUALIFICATION. The status endpoint reports installation separately from studio integration.

Magenta qualification covers text style and note guidance only. Audio-context encoding assets were deliberately excluded from the bounded download. Four-second offline throughput does not qualify live streaming, note/tempo adherence, human accompaniment, microphone feedback, or listening quality. The status endpoint reports model installation and studio controls separately. See MAGENTA-QUALIFICATION and its exact model/source identities.

Native realtime audio, Audio Unit/VST3/CLAP hosting, secure third-party plugin execution, large song-model workers, score-conditioned song editing, choir rendering, and Windows packaging remain roadmap work. No paid service, model gate acceptance, deployment, or personal-audio upload was performed.

The Mac was locked when native UI access was attempted. Continue software work without retrying the locked desktop. Native launch, microphone capture, physical device checks, and the native file-dialog/download path remain separate acceptance work.

The archive UI file chooser opens, but the automation extension cannot select a local file without its file URL permission. The same limitation applies to selecting an external audio file. The example button exercises the real import pipeline with a local generated Blob; it does not close the file-selection gate. No extension permissions were changed. Continue independent work rather than repeatedly retrying this gate. Archive tests and the actual downloaded bundle receipt are in `verification/archive-roundtrip.json`.

Recording recovery retains unsaved bytes in browser memory. It is not crash-durable pending-take storage: retry saving or download before closing. A recorder interruption retains the bytes it produced, without claiming that every interrupted container is playable. Codec decoding, real microphone privacy permission, and physical listening remain acceptance work. Receipts are in `verification/audio-import.json` and `verification/audio-recovery.json`.

## Overnight continuation

The `OmniuSong overnight build` heartbeat is active every 30 minutes through 8:00 AM America/Los_Angeles on October 15. It resumes this chat and reads this file and IMPLEMENTATION-PLAN before choosing the next bounded slice. It should update this status after meaningful completed work and leave a morning review.

The continuation should preserve original performances, avoid paid or externally published operations, keep runtime environments isolated, and report actual inference separately from installation. Stop starting work at the morning cutoff.

## Studio preview

![OmniuSong studio with the accepted arrangement and Basic Pitch candidate](verification/studio-preview.png)

![Saved-session selection and complete backup controls](verification/session-backup-preview.png)

![Studio with sustain and active-note controls](verification/input-preview.png)


![Imported original and accepted local transcription](verification/audio-import-preview.png)

![Aria piano preview with source and AI answer](verification/aria-preview.png)

![Aria studio workflow with accepted answer and bar-five review](verification/aria-studio.png)

![Magenta generated-audio preview with retained note guide](verification/magenta-studio.png)

![Magenta candidates and aligned Basic Pitch estimates](verification/magenta-candidates.png)


![Exact note controls with saved original values](verification/note-editing.png)


![Source-preserving audio clip controls and aligned local transcription](verification/clip-editing.png)
