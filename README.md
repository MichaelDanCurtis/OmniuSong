# OmniuSong

A Mac-first music workstation where people play, sing, edit, and direct multiple AI models. Windows follows through the same portable project format and model adapters.

The first development preview includes a playable studio, sixteen pads, a piano roll, vocal take capture, local projects, a model observatory, and generation candidates that require acceptance before they change the song. Basic Pitch performs real local audio-to-MIDI inference. Aria MLX creates local piano continuations from selected source bars, with separate audition, acceptance, and discard. The piano roll navigates the complete session and MIDI pitch range, with exact pitch, start, length, and 16-bit velocity controls. Original note values can be restored after saving or reloading. Arrangement and sample tools use musical rules and synthesis. Magenta Small generates short audio candidates from style prompts and optional note guides, with explicit placement, audition, acceptance, and discard. The Ollama lyric adapter can use an installed local AI model. Session playback also has a metronome and one- or two-bar count-in for armed keys/pads; candidate auditions cannot record over the source. The remaining researched models are candidates for integration.

## Start the studio

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
4. Record an original vocal take using **Record voice**. Recording requests microphone permission only when used. The full original take is available in its audio player and through **Download original**; new takes initially start at the beginning of the loop. Select the audio track to move it with **Start** or choose **Source in/out** trims, then choose **Save clip**. **Audition clip** previews the selected window; **Restore original clip** restores its original settings after reload. See [clip editing](docs/CLIP-EDITING.md). You can also **Import audio** from WAV, MP3, M4A, FLAC, Ogg, or WebM when the browser can read it, up to two minutes and 30 MiB. **Import example phrase** uses a synthesized four-tone fixture so you can try the complete flow immediately.
   Select a vocal or sample track and choose **Turn selected audio into notes** to run local Basic Pitch transcription. The selected audio trim is transcribed and its notes keep the clip position. Audition and accept the estimates separately; the original audio stays in the project.
   If a take cannot save, its original stays in the tab with **Retry saving take** and **Download unsaved original**. Session switching stays blocked until it is resolved. See [recording and import](docs/RECORDING.md) for limits.
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
