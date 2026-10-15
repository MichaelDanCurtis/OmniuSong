# OmniuSong implementation plan

October 15, 2026. Development home: `/Volumes/Main External/OmniuSong`.

## Current technical decisions

Use a dependency-free Node 22 control service and portable browser UI for the first executable slice. Use a small Swift macOS shell for CoreMIDI access. Version project data and model capabilities before adding heavy inference runtimes. Keep playback replaceable so a native audio engine can take over without translating saved songs.

The first implementation is intentionally a development system. The production audio engine, plugin isolation, standard MIDI 2 files, and qualified model workers are separate engineering work. The current preview must report their status accurately.

## Repository map

| Location | Responsibility |
| --- | --- |
| `src/project.mjs` | Project and note validation, IDs, non-destructive candidate merge |
| `src/midi.mjs` | SMF MIDI 1 output, MIDI 2 note and per-note pitch UMP, trace output |
| `src/storage.mjs` | Revision checks, serialized saves, atomic replacement, content-addressed audio |
| `src/take-recovery.mjs` | Bounded completed-original journals, restart recovery, source identity checks, and verified acknowledgment |
| `src/archive.mjs` | Complete project backup, bounded decoding, audio checksums, restore into a new session |
| `src/audio-generation.mjs` | Style/note scope, exact projected guide, clip placement and candidate metadata |
| `src/magenta.mjs` and `workers/magenta_worker.py` | Bounded offline Magenta Small audio generation, text/MIDI guidance, checksummed model assets and PCM output |
| `src/continuation.mjs` | Aria source scope, piano projection, response placement, bounded loop expansion |
| `public/transport.js` | Validated session click/count-in settings and one audio-clock boundary for playback and note capture |
| `public/clip-edit.js` | Pure audio placement/trim/restoration, preserved original settings, and checked decoded analysis windows |
| `public/note-edit.js` | Pure note edit/restore/delete transactions, original snapshots, expression-tail guards, and journal |
| `public/note-view.js` | Full-session editor pages and non-mutating candidate audition |
| `src/plugins.mjs` | Versioned developer registry, trusted built-ins, local Ollama adapter |
| `src/jobs.mjs` | Durable queue, cancellation, crash interruption, candidate review |
| `src/composer.mjs` and `src/wav.mjs` | Explicitly procedural musical sketch and sample tools |
| `src/server.mjs` | Loopback API, static studio, bounded uploads, export routes |
| `public/recording.js` | Permission/preparation/capture lifetimes, cancellation, retained originals, retry, and validated uncalibrated clock estimates |
| `public/input.js` | Portable source-aware voice lifetimes, sustain, MIDI 1/UMP decoding, and bounded raw capture |
| `public/` | Arrangement, piano roll, pads, keyboard, co-producer, lyric editing, model observatory |
| `catalog/models.json` | Sourced research candidates and initial Keep/Explore/Skip recommendations |
| `native/` | Mac shell and CoreMIDI UMP input |
| `src/worker.mjs` and `workers/` | Bounded local Basic Pitch process, ONNX inference, raw pitch estimates, reproducible environment |
| `scripts/` | Build, launch, verification, procedural example rendering |
| `test/` | Project, MIDI, persistence, queue, and HTTP integration checks |

## Initial build checklist

- [x] Establish an independent project and research catalog.
- [x] Define and validate integer musical timing, 16-bit velocities, and expression lanes.
- [x] Implement versioned registry and cancellable local generation jobs.
- [x] Preserve original tracks and require explicit candidate acceptance.
- [x] Implement revision conflicts, atomic project replacement, and previous-revision backup.
- [x] Build the playable studio, note grid, sample instrument, vocal capture flow, and model picker.
- [x] Implement MIDI 1 output and a clearly labeled UMP JSON trace.
- [x] Add the native Mac input shell and local application build.
- [x] Verify browser arrangement, sample, note editing, model inference, candidate acceptance, and saving.
- [x] Complete real Basic Pitch ONNX inference and preserve raw pitch estimates.
- [x] Implement complete project archives and session switching; verify downloaded backup restoration through the local service.
- [ ] Verify native launch after the Mac is unlocked.
- [ ] Complete real controller and microphone acceptance independently.

## Overnight continuation order

Read STATUS and the working tree before each continuation. Choose one bounded improvement, complete its implementation, run meaningful checks, and update the status with its practical limits. Prefer fixing musical data loss or broken flows over adding more model names.

1. **Finish the full session loop.** Exercise arrangement generation, audition, acceptance, note editing, save/reload, sample playback, exports, model selections, native launch, and input connection. Fix discovered errors.
2. **Portable archives and session switching.** Implemented: checksummed `.omniusong` bundles include all referenced audio, and restore creates a new session. HTTP round trips and the live downloaded bundle pass; browser switching saves edits and remembers the selected session. Automated browser file selection remains unverified because the Chrome extension lacks file URL permission. Native Mac file dialogs and download handling still need qualification.
3. **Input reliability.** Implemented and software tested: source/group/channel identities, UUID voice lifetimes, sustain, fast release during audio startup, focus loss, panic, device loss, release velocity, and bounded original UMP/MIDI 1 capture. Browser sustain/record/save/reload is verified. Native virtual-endpoint and physical device qualification remain open.
4. **Extend transcription qualification.** Audio import and recording-state safeguards are implemented. The browser example exercises import, actual Basic Pitch inference, acceptance, playback, and complete backup with original bytes preserved. Physical microphone operation, file-chooser selection, other codec decoding, and human singing/instrument quality remain open. Retain raw estimates and source audio. Compare MuScriptor for mixed material without treating isolated-tone success as general transcription accuracy.
5. **Aria continuation integration (implemented).** Actual MLX inference now runs through the durable candidate queue with source-bar selection, response token budget, seed, separate audition/accept/discard, and safe loop expansion. Full-session bar pages and MIDI pitch navigation support review. The locked source, model evidence, and continuation survive backups. Two seeded browser candidates differ; discard, acceptance, bar-five edit/undo, and reload are verified. Magenta RealTime 2 Small is now independently qualified for short offline stereo generation with text and optional note guidance. Two four-second outputs and a saved studio preview are verified; source and audio survive complete backups. Magenta audio candidates are now integrated with source scope, 2/4/8-second duration controls, saved clip placement, audition/accept/discard and complete backup evidence. Basic Pitch preserves the position when transcribing placed audio. Precise human note controls are now implemented: pitch/start/length/16-bit velocity, preserved originals, expression-tail guards, locked inspection, repeated-note selection, and browser/backup/export evidence. Audio clip reposition/trim controls and source-window transcription are also implemented and browser verified, with original source bytes and settings retained. Metronome/count-in and isolated non-recording auditions are now implemented and browser verified. Song-mode capture and saved recorder-start timing estimates are now wired and software tested; chosen-bar imports and review/restoration are browser verified. Completed-original journals now recover after reload/service restart without duplicate tracks; known unattached originals block incomplete project backups. Recovery entries can now be reversibly set aside and returned, preserving raw journals/source files and keeping project backups explicit. Next add non-destructive clip fades and clearer audio-window review, then qualify microphone capture and measured latency when the desktop is available; incremental capture and streaming are separate.
6. **Model worker contract.** Create a typed local worker protocol, health and model inventory endpoints, parameter schema, cancellation, result checksums, and installation states. Implement one official runtime's actual API; avoid placeholder endpoints.
7. **Recording and expression editing.** Saved clip offsets, visible arm state, and exact note pitch/timing/velocity edits are implemented. Original note snapshots persist across reload and archives; general Undo remains tab-local. Audio clip reposition/trim and original-settings restoration are also implemented; transcription records its selected source frames. Metronome/count-in, cancelable startup and non-recording auditions are implemented. Completed-original recovery journals and browser reload/service restart recovery are also implemented; active recording fragments still depend on the tab. Add expression curves, clip fades/waveform handles, measured voice recording alignment, and a reviewable quantization operation. Sing with song now saves an uncalibrated browser clock receipt and initial placement/trim; physical microphone acceptance remains open.

## Model adapter contract

A task includes `apiVersion`, `taskId`, `projectId`, `projectRevision`, `scope`, `capability`, `modelRef`, `inputAssets`, `musicalContext`, `parameters`, `seed`, and `limits`. A model reference includes repository/provider identity, exact revision, weight hashes when available, license source, and adapter version.

The worker reports health separately from model installation, checkpoint loading, and generation availability. Progress uses phase names and measured progress where the runtime provides it. A result declares genuine outputs and includes their type, checksum, timing, metadata, and provenance. Never synthesize an output stem label for a stereo mix or claim unsupported hard constraints.

The first in-process registry demonstrates the extension boundary. Before accepting third-party executable packages, replace it with a worker supervisor that enforces resource and permission boundaries. Manifest validation alone is not sandboxing.

## Data evolution

Keep schema migrations explicit and reversible. Input source/address, release velocity, internal note identity, source receipt timing, raw UMP/MIDI 1 capture, versioned original-note edit snapshots, audio trim windows, original-clip setting snapshots, versioned metronome/count-in preferences, and uncalibrated recording-clock receipts are optional validated fields in schema 1. Sample-accurate offsets, tempo maps, clock calibration, and an independently qualified native input/output path remain necessary before claiming complete MIDI 2 recording. Unknown model metadata must survive round trips.

Use project transactions for edits. Store a compact operation journal and checkpoint snapshots once durable undo lands. A cross-file candidate acceptance should eventually be one transaction; the first queue uses an acceptance-history marker to recover the project/job split safely.

## Verification sequence

Run syntax and catalog checks, focused unit and integration tests, and the build affected by a change. Inspect the actual studio after UI changes. Tests for note packet layouts, stale candidate conflicts, cancellation, and crash recovery matter more than implementation-mirroring UI snapshots.

For each real model, retain a reproducible musical brief, exact model revision, runtime version, hardware, elapsed time, memory observation, generated output, and a listening decision. Compare at least a human original and two candidates where quality is subjective.

For MIDI and audio hardware, retain device model, firmware where known, driver/runtime, buffer size, sample rate, observed events, audible output, recorded file, and physical acceptance. Software-only checks cannot close those gates.

## Overnight operating limits

Work only in the OmniuSong project unless a needed dependency is explicitly shared. Preserve unrelated work. No deployment, purchases, paid API calls, uploads of personal recordings, voice training, or hardware firmware changes are needed for the overnight local build. Do not search unrelated credentials to enable an adapter.

Keep heavy runtimes isolated. Inspect disk and memory before downloads; prefer bounded experiments and stop expensive work that threatens the active desktop. If gated weights require account acceptance, document the exact model and continue independent work. Do not bypass the gate.

At 8:00 AM America/Los_Angeles on October 15, stop scheduling new overnight work and leave a concise review note with actual completed features, verification, unfinished work, and the next best musical test.
