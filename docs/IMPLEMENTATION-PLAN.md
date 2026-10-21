# OmniuSong implementation plan

The active October 20, 2026 implementation is the [native Rust migration plan](RUST-MIGRATION.md), governed by the [complete goal](GOAL.md), [architecture](RUST-ARCHITECTURE.md) and [release acceptance matrix](RELEASE-ACCEPTANCE.md). Repository: `/Volumes/Main External/Development/OmniuSong`. The original overnight plan below is retained as historical reference; its Node/browser/Swift runtime decisions do not govern the production application.

## Current native delivery boundary

The [arrangement source gate](verification/rust-native-arrangement-tests.json) passes **562 entries** and formatting/Clippy/build/package/signature checks. Its **09789029…** bundle now completes the [bounded actual walkthrough](verification/rust-native-arrangement-ui.json): exact drag/nudge/snap, explicit Save/Discard, guarded combined drafts, history, complete backup/restore, same-binary restart/reopen and 48 kHz speaker output/Stop. All fourteen earlier sessions and original objects remain exact; two fresh synthetic copies are added. The installed bundle remains this qualified arrangement build.

The [incremental performance writer](contracts/NATIVE-PERFORMANCE-WRITER.md) passes its [636-entry source/software qualification](verification/rust-native-performance-writer.json), final Clippy/build, eight actual append-boundary helper exits and a 38.4 MB / 4,160-event six-stream recovery/publication/archive proof. Journal version 1 is unchanged; selected checkpoint recovery preserves partial/bad tails. This remains a library/source packet: the installed 0978 bundle is unchanged, and no native recording command, normal Finish or recording UI is delivered by it. Follow the [capture architecture](reviews/RUST-PERFORMANCE-CAPTURE-ARCHITECTURE.md) for worker/UI lifecycle and derived editing.

The [waveform source gate](verification/rust-native-waveform-tests.json) now passes **517 entries**, with the **9b2fb28c…** development package and its [bounded actual native walkthrough](verification/rust-native-waveform-ui.json). Source/current/original/local overlays, draft/selection isolation, corrected full-lane layout, output-control availability and same-binary restart pass. All 14 projects / 220 catalog rows, four immutable takes, catalog bytes and 30 prototype files remain exact.

The [earlier clip-editor source gate](verification/rust-native-clip-editor-tests.json) records **469 passing workspace entries**, zero failures and two intentionally ignored subprocess entrypoints, with formatting/Clippy, production workspace/all-target build and the **216d8e1c…** Mac development package verified. The [actual bounded clip walkthrough](verification/rust-native-clip-editor-ui.json) covers precise settings/drafts/history/originals, corrected range rejection, render changes, complete archive/copy, process exit/restart/reopening and native speaker-output completion. Older 425/394-entry gates and 85ef note UI retain their original boundaries. Seven Rust crates cover the native app, transactional storage, journals/IPC and audio process. Native input remains unexercised under the user's software-loopback-only instruction.

The [final-source internal software bus](verification/rust-native-audio-software-bus-final.json) retains 108,000 stereo frames at 48 kHz exactly through capture, explicit sealing, attachment, Undo/Redo, missing-stage replay, archive and restore. This is offline synthetic routing, without native input, listening or elapsed-device-capture acceptance. The [independent review](reviews/RUST-AUDIO-ENGINE-INITIAL.md) covers fixed callback data paths, exact overflow/unknown-tail distinctions, worker loss and the pinned dependency repair. The [preservation audit](verification/rust-native-audio-preservation.json) retains all six earlier native sessions and all 30 original prototype files unchanged.

Schema-4 storage, background session actions, paired name/tempo editing, OSARv2 archives, strict v1 reads and restore-as-copy remain implemented. The [earlier actual Mac walkthrough](verification/rust-native-disconnect-ui.json) completed two copy generations, paired Save/Undo/Redo, malformed-backup rejection and process exit/relaunch/reopen; it names the earlier 191-entry binary. The [drive-disconnect recovery](DISCONNECT-RECOVERY.md) retains three complete projects and one explicit fresh snapshot. One history row and three receipts remain missing; none were fabricated.

Full milestone 2 and every release row remain incomplete. The cb64 actual output/Stop and recovery/archive walkthrough is complete; the corrected live-focus build’s bounded interaction/reopen evidence now passes. Retained-stage discovery, read-only review, explicit checked recovery, separate attachment and keep-local/typed-conflict choices are now implemented and covered by the [recovery qualification](verification/rust-native-capture-recovery.json) and [independent review](reviews/RUST-CAPTURE-RECOVERY-INITIAL.md). The isolated fixture preserves 12,000 verified frames and its damaged tail through a complete project archive. Live keys/pads source and software gates now pass. The finite note backend and bounded actual editor walkthrough now pass; the clip backend and native clip inspector pass their bounded qualifications. Next packets: saved performance capture, buffer/clock visibility, metronome/count-in and overdub/compensation; extend recovery with reversible set-aside and combined unassigned-source export. A completed malformed-backup error followed by successful export now visibly clears stale status. Prior inspection/current receipt wording and a heartbeat-error reporting fix pass the latest source gate; the revised recovery-flow wording still needs its own actual UI check. Preserve separate hardware/listening, MIDI 2/MPE/Clip File, inference, plugin, installer and Windows gates.

## Next native work packet

The bounded **85ef6cd7…** [actual note-editor walkthrough](verification/rust-native-note-editor-ui.json) is complete: exact edits/selection/drafts/history/originals, expression and format rejection/review, committed offline render changes, edited archive/copy, verified process exit/restart/reopening and native speaker-output completion. Its build remains tied to the earlier 394-entry source/signature packet, with no inference or input claim. All ten previous projects and originals remain exact in the [audit](verification/rust-native-note-editor-preservation.json).

The [clip backend contract](contracts/NATIVE-CLIP-EDITING.md) is implemented and independently reviewed, with a [425-entry source gate](verification/rust-native-clip-editing-tests.json). It keeps legacy seconds/ticks and native frame units distinct, retains originals, authenticates current/original source windows and preserves durable archive/history/retry semantics. The later 216d clip-inspector bundle now replaces the app at the primary development-bundle path; the builder retained the earlier 85ef bundle separately.

The [216d native clip packet](verification/rust-native-clip-editor-ui.json) now completes that bounded delivery, including the light-theme correction, three-family headless guards and actual setting/history/archive/render/restart/output checks. The [native source waveform packet](verification/rust-native-waveform-ui.json) now completes its bounded read-only overview, with independent bin/authentication checks and actual corrected-package source/draft/selection/restart/output evidence. The source-preserving arrangement now completes its bounded exact-binary walkthrough. Saved performance capture now has a qualified incremental writer and explicit checkpoint recovery. Add worker/UI Start/Finish/Stop with a real callback fence and native closure lifetime, then complete original attachment and a separate derived editable-note projection. Keep full original audio and events through every stage. Then add metronome/count-in, buffer/clock visibility and monitoring/overdub/compensation. Waveform dragging/zoom and sustained background/output qualification remain separate. Keep retained-stage set-aside/unassigned-source export and catalog/volume fault work explicit. Full milestone 2 remains incomplete; preserve separate MIDI 2/MPE, physical input/listening, inference, plugin, installer and Windows gates.

The remaining saved-performance work follows this dependency order:

1. **Backend qualified:** declared original streams, atomic complete groups, bounded checkpoints and exact source/clock metadata. Integrate one off-callback draining owner with early stage identity and safe failure retirement.
2. Publish stage identity before streaming; expose explicit worker/UI Start, Finish, Stop and retained-prefix recovery without automatic sealing or replay.
3. Attach originals with durable intent, Undo/Redo and complete project/archive closure. Preserve the source PCM and events independently of edits.
4. Offer an explicit atomic editable-note projection with provenance and original references; disclose 960 PPQ rounding and the differing live/saved procedural instruments.

## Historical prototype plan

October 15, 2026. Development home: `/Volumes/Main External/OmniuSong`.

## Prototype technical decisions (historical)

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
| `public/clip-waveform.js` | Bounded multichannel peak summaries, source-window overview and stale decode isolation |
| `public/clip-edit.js` | Pure audio placement/trim/fade/restoration, preserved original settings, and checked decoded analysis windows |
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
5. **Aria continuation integration (implemented).** Actual MLX inference now runs through the durable candidate queue with source-bar selection, response token budget, seed, separate audition/accept/discard, and safe loop expansion. Full-session bar pages and MIDI pitch navigation support review. The locked source, model evidence, and continuation survive backups. Two seeded browser candidates differ; discard, acceptance, bar-five edit/undo, and reload are verified. Magenta RealTime 2 Small is now independently qualified for short offline stereo generation with text and optional note guidance. Two four-second outputs and a saved studio preview are verified; source and audio survive complete backups. Magenta audio candidates are now integrated with source scope, 2/4/8-second duration controls, saved clip placement, audition/accept/discard and complete backup evidence. Basic Pitch preserves the position when transcribing placed audio. Precise human note controls are now implemented: pitch/start/length/16-bit velocity, preserved originals, expression-tail guards, locked inspection, repeated-note selection, and browser/backup/export evidence. Audio clip reposition/trim controls and source-window transcription are also implemented and browser verified, with original source bytes and settings retained. Metronome/count-in and isolated non-recording auditions are now implemented and browser verified. Song-mode capture and saved recorder-start timing estimates are now wired and software tested; chosen-bar imports and review/restoration are browser verified. Completed-original journals now recover after reload/service restart without duplicate tracks; known unattached originals block incomplete project backups. Recovery entries can now be reversibly set aside and returned, preserving raw journals/source files and keeping project backups explicit. Non-destructive linear clip fades are now implemented, browser verified and retained in complete backups. A real source-waveform overview now previews pending trim/fade fields without saving them. Next qualify microphone capture and measured latency when the desktop is available; incremental capture and streaming are separate.
6. **Model worker contract.** Create a typed local worker protocol, health and model inventory endpoints, parameter schema, cancellation, result checksums, and installation states. Implement one official runtime's actual API; avoid placeholder endpoints.
7. **Recording and expression editing.** Saved clip offsets, visible arm state, and exact note pitch/timing/velocity edits are implemented. Original note snapshots persist across reload and archives; general Undo remains tab-local. Audio clip reposition/trim and original-settings restoration are also implemented; transcription records its selected source frames. Metronome/count-in, cancelable startup and non-recording auditions are implemented. Completed-original recovery journals and browser reload/service restart recovery are also implemented; active recording fragments still depend on the tab. Linear clip fades are now implemented. A read-only waveform overview is also implemented. Add expression curves, waveform handles, measured voice recording alignment, and a reviewable quantization operation. Sing with song now saves an uncalibrated browser clock receipt and initial placement/trim; physical microphone acceptance remains open.

## Model adapter contract

A task includes `apiVersion`, `taskId`, `projectId`, `projectRevision`, `scope`, `capability`, `modelRef`, `inputAssets`, `musicalContext`, `parameters`, `seed`, and `limits`. A model reference includes repository/provider identity, exact revision, weight hashes when available, license source, and adapter version.

The worker reports health separately from model installation, checkpoint loading, and generation availability. Progress uses phase names and measured progress where the runtime provides it. A result declares genuine outputs and includes their type, checksum, timing, metadata, and provenance. Never synthesize an output stem label for a stereo mix or claim unsupported hard constraints.

The first in-process registry demonstrates the extension boundary. Before accepting third-party executable packages, replace it with a worker supervisor that enforces resource and permission boundaries. Manifest validation alone is not sandboxing.

## Data evolution

Keep schema migrations explicit and reversible. Input source/address, release velocity, internal note identity, source receipt timing, raw UMP/MIDI 1 capture, versioned original-note edit snapshots, audio trim windows, optional linear clip fades, original-clip setting snapshots, versioned metronome/count-in preferences, and uncalibrated recording-clock receipts are optional validated fields in schema 1. Sample-accurate offsets, tempo maps, clock calibration, and an independently qualified native input/output path remain necessary before claiming complete MIDI 2 recording. Unknown model metadata must survive round trips.

Use project transactions for edits. Store a compact operation journal and checkpoint snapshots once durable undo lands. A cross-file candidate acceptance should eventually be one transaction; the first queue uses an acceptance-history marker to recover the project/job split safely.

## Verification sequence

Run syntax and catalog checks, focused unit and integration tests, and the build affected by a change. Inspect the actual studio after UI changes. Tests for note packet layouts, stale candidate conflicts, cancellation, and crash recovery matter more than implementation-mirroring UI snapshots.

For each real model, retain a reproducible musical brief, exact model revision, runtime version, hardware, elapsed time, memory observation, generated output, and a listening decision. Compare at least a human original and two candidates where quality is subjective.

For MIDI and audio hardware, retain device model, firmware where known, driver/runtime, buffer size, sample rate, observed events, audible output, recorded file, and physical acceptance. Software-only checks cannot close those gates.

## Overnight operating limits

Work only in the OmniuSong project unless a needed dependency is explicitly shared. Preserve unrelated work. No deployment, purchases, paid API calls, uploads of personal recordings, voice training, or hardware firmware changes are needed for the overnight local build. Do not search unrelated credentials to enable an adapter.

Keep heavy runtimes isolated. Inspect disk and memory before downloads; prefer bounded experiments and stop expensive work that threatens the active desktop. If gated weights require account acceptance, document the exact model and continue independent work. Do not bypass the gate.

At 8:00 AM America/Los_Angeles on October 15, stop scheduling new overnight work and leave a concise review note with actual completed features, verification, unfinished work, and the next best musical test.
