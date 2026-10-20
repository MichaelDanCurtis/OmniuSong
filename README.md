# OmniuSong

A Mac-first music workstation where people play, sing, edit, and direct multiple AI models. Windows follows through the same portable project format and model adapters.

## Native Rust migration

The active [release goal](docs/GOAL.md) requires a native Rust application on Mac and Windows, including MIDI 2.0 and MPE. Follow the [Rust architecture](docs/RUST-ARCHITECTURE.md), [migration plan](docs/RUST-MIGRATION.md) and [release acceptance matrix](docs/RELEASE-ACCEPTANCE.md). Existing browser/Swift/Python components below are reference implementations, not the production runtime.

The Rust workspace contains the lossless schema-1 bridge, explicit schema-2 recording references, a separate transactional SQLite store, native egui/eframe interface, immutable take journals/owned snapshots, complete retained-project archives, a bounded background session service and private Unix worker IPC. The seventh crate adds a bundled native audio process with device selectors, saved-session Play/Stop, supported procedural notes/WAV/sealed-source playback and solo-input capture/Finish. See the [audio contract](docs/contracts/NATIVE-AUDIO-ENGINE.md) for exact bounds and failure behavior.

Build the Mac development bundle with `bash scripts/build-native-mac.sh`, then open `dist/OmniuSong Native.app`. It requires no local server. Native data uses `.runtime/native-studio/`; existing `data/` sessions are read-only import sources. Command-line `--data-dir` and `--legacy-dir` overrides are available. The [earlier rebuilt Mac UI](docs/verification/rust-native-disconnect-ui.json) opens the recovered store and has exercised backup export, inspection, restore-as-copy, paired name/tempo Save/Undo/Redo, a second-generation copy, malformed-backup rejection and actual process exit/relaunch/reopen. Saved recording ranges and the incomplete-history warning persist. Earlier binaries separately verified create/import/discard/reopen and read-only schema-2 recording inspection. The current audio-enabled development build has a responsive loading window but is awaiting macOS external-drive access before its device/output UI check. Native input remains unexercised under the user's software-loopback-only instruction; model/plugin controls remain unavailable. See [current status](docs/STATUS.md).

All 19 original sessions have passed native import/reopen with exact source JSON and referenced audio retained. All 30 original project/audio files still match the initial inventory. The native store is schema 4. [Native archives](docs/contracts/NATIVE-ARCHIVE.md) retain originals referenced only by historical operation results and restore identities without overwriting existing projects. Hash-bound restore-as-copy and complete immutable ancestry are implemented, with strict OSAR version-1 reading and version-2 exports. Background actions and backup controls pass service/native UI tests. Bounded retained-stage scan, read-only inspection, explicit verified-prefix recovery and separate attachment are implemented. General set-aside and combined unassigned-source archives remain pending.

The October 20 external-drive disconnect damaged the development catalog. All four current documents survived: three complete projects were restored exactly, and the fourth became a fresh revision-0 recovery fork with its exact source/evidence embedded. One historical row and three old operation receipts were lost; none were fabricated. The raw damaged store remains separately retained. [Recovery and promotion evidence](docs/DISCONNECT-RECOVERY.md) describes the bounded operator procedure. Typed SQLite I/O/corruption/not-a-database failures now retire the worker generation, and explicit existing-only reconnect cannot create a replacement catalog. Late wrapped/per-project errors, stalled I/O deadlines, volume enrollment, general catalog recovery UI and physical power-loss durability remain gaps.

Rust checks: `cargo fmt --all -- --check`, `cargo test --workspace --all-features --locked`, and `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`. `--all-features` includes recording process-interruption fixtures through the test-hook feature; ordinary development bundles do not enable it.

The [current Rust receipt](docs/verification/rust-native-capture-recovery-tests.json) records **278 passing workspace entries**, zero failures and one intentionally ignored subprocess helper, plus five focused CoreAudio patch tests. Formatting/Clippy, Mac build and development signature pass. The [software-bus qualification](docs/verification/rust-native-audio-software-bus-final.json) proves exact synthetic PCM through capture, sealing, attachment, Undo/Redo, retry and complete archive restoration; it opens no native input and is not elapsed or physical recording. The live **Native software loopback · recorded original** example retains the earlier identical PCM result. The [preservation audit](docs/verification/rust-native-audio-preservation.json) confirms the six previous native sessions and all 30 prototype files remain unchanged.

The [recovery qualification](docs/verification/rust-native-capture-recovery.json) recovers 12,000 synthetic frames from a journal with a damaged tail. Original bad rows, interruption evidence and Undo/Redo history survive a 249,856-byte archive and restoration exactly. The [new preservation audit](docs/verification/rust-native-capture-recovery-preservation.json) confirms all eight earlier native sessions and 30 prototype files remain unchanged. [Independent recovery review](docs/reviews/RUST-CAPTURE-RECOVERY-INITIAL.md) and 34 headless app cases cover explicit recovery, stable retries and conflict release; actual Mac controls remain awaiting external-drive access.

The [independent audio review](docs/reviews/RUST-AUDIO-ENGINE-INITIAL.md) includes fixed-queue/clock/stop failure cases and the narrow vendored CoreAudio input repair. Actual input, listening, latency, thirty-minute stability and Windows are separate gates. No resampling, live keyboard/pads, monitoring/overdub/count-in/compensation yet. The [retained-capture workflow](docs/contracts/NATIVE-CAPTURE-RECOVERY.md) now supports explicit review/recovery/attachment and safe keep-local/conflict choices; actual rebuilt UI confirmation remains pending. Full milestone 2 and every release row remain incomplete. MIDI 2 project data, UMP, native transport, MIDI-CI, standard Clip Files and MPE remain separately tracked; no native MIDI delivery is implied by audio work.

Earlier [191-entry disconnect/UI](docs/verification/rust-native-disconnect-tests.json), [176-entry session/copy](docs/verification/rust-native-session-copy.json) and [152-entry take/archive](docs/verification/rust-native-takes.json) receipts remain evidence for their exact older binaries. The [copy qualification](docs/verification/rust-native-archive-copy.json) retains complete v1→v2→copy-of-copy ancestry. Full native inference, instruments/plugins, choir, installers and Windows acceptance remain required.

## Existing prototype reference

The capabilities below describe the earlier prototype. A fresh October 20 baseline passes 114 of 116 tests: Basic Pitch inference works; Aria and Magenta currently fail imports because their Python editable paths refer to the old checkout location. Their prior qualification receipts remain historical. See [baseline evidence](docs/verification/rust-baseline-tests.json).

The first development preview includes a playable studio, sixteen pads, a piano roll, vocal take capture, local projects, a model observatory, and generation candidates that require acceptance before they change the song. Basic Pitch performs real local audio-to-MIDI inference. Aria MLX creates local piano continuations from selected source bars, with separate audition, acceptance, and discard. The piano roll navigates the complete session and MIDI pitch range, with exact pitch, start, length, and 16-bit velocity controls. Original note values can be restored after saving or reloading. Arrangement and sample tools use musical rules and synthesis. Magenta Small generates short audio candidates from style prompts and optional note guides, with explicit placement, audition, acceptance, and discard. The Ollama lyric adapter can use an installed local AI model. Session playback also has a metronome and one- or two-bar count-in for armed keys/pads and the optional Sing with song recording mode; candidate auditions cannot record over the source. The remaining researched models are candidates for integration.

## Start the prototype studio

Requires Node 22 or newer. The portable studio has no package dependencies.

```sh
npm start
```

Open [the local studio](http://127.0.0.1:43178). On Mac, double-click `scripts/start-mac.command`. A compiled Mac preview is placed in `dist/OmniuSong.app` by:

```sh
npm run mac:build
```

The development app requires the local server to be running. It adds CoreMIDI UMP input to the portable studio. It is unsigned and is not an installer or a production audio engine.

## Try a complete session

1. Choose **Suggest an arrangement**, audition the candidate, then accept it.
2. Select a track and click an empty piano-roll cell to add a note. Click a note to inspect it; use **Selected note** to reach repeated notes in the same cell. Edit pitch, start, length, and velocity, then press Enter or choose **Save note**. Moving to another note, track, or session also saves pending note edits. **Restore original** reverses these edits even after reload. See [note editing](docs/NOTE-EDITING.md). To capture a performance, arm the track with **R**, choose **Count-in** if desired, then use **Play** and begin after the countdown. Toggle **Metronome** and adjust **Click level** while playing. Auditions do not record into armed tracks. See [transport and count-in](docs/TRANSPORT.md). A through K play the white keys when a text field is not focused. Use **Sustain** for the on-screen or computer keys, or CC64 on your MIDI pedal. **All notes off** clears held notes and sustain. New performances retain their timing without grid quantization.
3. Choose **Make a glass instrument**, audition the sample, accept it, select its track, and play it on the keyboard.
4. Record an original vocal take using **Record voice**. Recording requests microphone permission only when used. The full original take is available in its audio player and through **Download original**; solo takes and imports use **Solo voice / imports at bar**. Enable **Sing with song** to request permission first, load the backing song, and record with its count-in from bar one. The arrangement uses a visible, uncalibrated recorder-start timing estimate; the complete source includes the count-in. Review or adjust that estimate after listening. Physical microphone operation and latency remain unverified. Select the audio track to move it with **Start** or choose **Source in/out** trims and **Fade in/out** lengths, then choose **Save clip**. Fades shape arrangement playback and clip audition while preserving the complete source. **Show waveform** reviews the full source, selected window and fades, including a clearly labeled unsaved preview. **Audition clip** previews the selected window; **Restore original clip** restores its original settings after reload. See [clip editing](docs/CLIP-EDITING.md). You can also **Import audio** from WAV, MP3, M4A, FLAC, Ogg, or WebM when the browser can read it, up to two minutes and 30 MiB. **Import example phrase** uses a synthesized four-tone fixture so you can try the complete flow immediately.
   Select a vocal or sample track and choose **Turn selected audio into notes** to run local Basic Pitch transcription. The selected audio trim is transcribed and its notes keep the clip position. Audition and accept the estimates separately; the original audio stays in the project.
   If a take cannot save, use **Retry saving take** or **Download unsaved original**. Once the complete original and its local recovery record are stored, **Unfinished audio saves** can recover it after a reload/service restart without duplicates. Active capture before that boundary still depends on the tab. Session switching stays blocked while an in-tab take is pending; complete backup requires active recovery originals to be attached or explicitly set aside. **Set aside · keep locally** keeps an entry and its files visible on this Mac without blocking the song backup; **Return to recovery** reverses that choice. Project backups include their accepted tracks and referenced audio, while set-aside recovery files stay local. See [recording and import](docs/RECORDING.md) for limits.
5. Select a pitched note track and open **Continue with Aria · local AI**. Choose its start bar, 1–4 source bars (up to 15 seconds), an approximate answer length, and Seed. Choose **Continue selected piano phrase**, audition the answer, then accept or discard it. Acceptance adds a separate piano track after the selected bars and extends the loop if needed. Original notes and expression stay intact; Aria uses a cropped piano projection without expression. Use **Visible bars** and **Pitch range** to review the whole answer.
6. Open **Make audio with Magenta · local AI**. Describe the sound above, choose 2, 4, or 8 seconds, and choose **Place at bar**. Optionally select a pitched track and enable **Guide with selected notes**. Choose **Generate audio candidate**, audition it, then accept or discard. The audio becomes a separate track at the chosen bar; the loop expands if needed. Notes and tempo are guidance, not exact constraints.
7. Review the **Model observatory** and save Keep, Explore, or Skip choices.
8. Choose **Backup project** to download a complete `.omniusong` archive with its referenced audio. Use **Restore backup** to open it as a new session. Switch between saved work with **Saved sessions**; pending edits are saved before a switch.
9. Export a MIDI 1 file or MIDI 2 UMP packet trace. The packet trace is an OmniuSong JSON format; Standard MIDI Clip File export is planned.

To use a local AI lyric model, start an existing Ollama installation and set `OMNIUSONG_OLLAMA_MODEL` to an installed model before starting the studio. Then enable **Use my local AI lyric model**. The app does not install or select a model silently.

Basic Pitch is installed in this checkout's isolated `.runtime/basic-pitch` environment. To recreate it, use `scripts/setup-basic-pitch.sh` with uv and Python 3.11. This selects the ONNX runtime and avoids installing TensorFlow or CoreML. Exact resolved package versions are recorded in `workers/basic-pitch-resolved.txt`. The synthetic four-tone qualification and model checksum are in `docs/verification/basic-pitch.json`.

Aria uses its own isolated `.runtime/aria` environment and a pinned 2.63 GB checkpoint. See [Aria qualification](docs/ARIA-QUALIFICATION.md) for the exact model, compatibility fix, measured fixture, setup, and MIDI/audio artifacts. The saved **Aria studio qualification** session demonstrates the complete flow with an unchanged locked procedural prompt, two different seeded answers (one discarded), and an accepted 21-note answer across five bars. Listening and human-performance quality checks remain open.

Magenta RealTime 2 Small uses a separate `.runtime/magenta` environment and 1.04 GB of selected, checksummed model assets. See [Magenta qualification](docs/MAGENTA-QUALIFICATION.md) for setup, actual audio results, note guidance, and limitations. Open **Magenta Small · local audio preview** to compare the style-only and note-guided renders. These are generated audio files, separately labeled from human recordings. The newer **Magenta studio qualification** session demonstrates an accepted eight-second clip at bar two, with its locked source notes unchanged. Transcription of placed audio keeps the estimated notes at that clip's position. The **Clip editing qualification** session demonstrates a moved 1.6-second source window, exact restoration of its complete 3.2-second original, and two actual Basic Pitch estimates from that window.

## Project and planning documents

- [Product specification](docs/PRODUCT-SPEC.md)
- [Model research and recommendations](docs/MODEL-RESEARCH.md)
- [Delivery roadmap](docs/ROADMAP.md)
- [Implementation plan](docs/IMPLEMENTATION-PLAN.md)
- [Hardware and MIDI design](docs/HARDWARE-MIDI.md)
- [Current implementation and verification](docs/STATUS.md)

Projects, jobs, audio assets, and model selections live in the ignored `data/` directory. Project saves retain one previous revision in `data/backups/`. Complete `.omniusong` backups include referenced audio, source recordings used for transcription, expression data, raw MIDI capture, and accepted-candidate provenance. Restore checks every audio checksum before publishing a new project and leaves existing sessions intact. See the [archive format and limits](docs/PROJECT-ARCHIVE.md).

## Verification

```sh
npm run check
npm test
npm run demo
```

The demo writes a procedural WAV, MIDI 1 file, project, and UMP trace under `dist/`. No AI model is used for that demo.

Cloud execution is disabled in this preview. Audio Units, VST3, CLAP hosting, production recording alignment, MIDI output, MIDI-CI, and standard MIDI 2 Clip Files remain roadmap work. Native keyboard input, microphone operation, and audible quality require device and listening acceptance in addition to software checks.
