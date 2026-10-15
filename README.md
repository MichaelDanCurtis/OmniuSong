# OmniuSong

A Mac-first music workstation where people play, sing, edit, and direct multiple AI models. Windows follows through the same portable project format and model adapters.

The first development preview includes a playable studio, sixteen pads, a piano roll, vocal take capture, local projects, a model observatory, and generation candidates that require acceptance before they change the song. Basic Pitch performs real local audio-to-MIDI inference. Arrangement and sample tools use musical rules and synthesis. The Ollama lyric adapter can use an installed local AI model. The other researched models are candidates for integration.

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
2. Select a track and click the piano roll. To capture a performance, arm the track with **R** and play keys while the transport runs. A through K play the white keys when a text field is not focused. Use **Sustain** for the on-screen or computer keys, or CC64 on your MIDI pedal. **All notes off** clears held notes and sustain. New performances retain their timing without grid quantization.
3. Choose **Make a glass instrument**, audition the sample, accept it, select its track, and play it on the keyboard.
4. Record an original vocal take using **Record voice**. Recording requests microphone permission only when used. The full original take is available in its audio player; arrangement playback currently places it at the start of the loop.
   Select a vocal or sample track and choose **Turn selected audio into notes** to run local Basic Pitch transcription. Audition and accept the estimated notes separately; the original audio stays in the project.
5. Review the **Model observatory** and save Keep, Explore, or Skip choices.
6. Choose **Backup project** to download a complete `.omniusong` archive with its referenced audio. Use **Restore backup** to open it as a new session. Switch between saved work with **Saved sessions**; pending edits are saved before a switch.
7. Export a MIDI 1 file or MIDI 2 UMP packet trace. The packet trace is an OmniuSong JSON format; Standard MIDI Clip File export is planned.

To use a local AI lyric model, start an existing Ollama installation and set `OMNIUSONG_OLLAMA_MODEL` to an installed model before starting the studio. Then enable **Use my local AI lyric model**. The app does not install or select a model silently.

Basic Pitch is installed in this checkout's isolated `.runtime/basic-pitch` environment. To recreate it, use `scripts/setup-basic-pitch.sh` with uv and Python 3.11. This selects the ONNX runtime and avoids installing TensorFlow or CoreML. Exact resolved package versions are recorded in `workers/basic-pitch-resolved.txt`. The synthetic four-tone qualification and model checksum are in `docs/verification/basic-pitch.json`.

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
