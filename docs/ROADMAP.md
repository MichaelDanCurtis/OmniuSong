# OmniuSong delivery roadmap

October 20 update: the [native Rust migration sequence](RUST-MIGRATION.md) is now the active dependency order. The [complete goal](GOAL.md) preserves the full music/model/plugin scope and requires both MIDI 2.0 and MPE on native Mac and Windows. Stage descriptions below remain capability context; preview completion does not close native release gates. Follow the [release acceptance matrix](RELEASE-ACCEPTANCE.md).

October 15, 2026. Sequence follows musical usefulness and architectural dependencies. Durations are planning ranges for a small focused engineering effort, not completion promises.

## Stage 0 Research and a playable foundation

**First overnight slice.** Establish the project, compare models, and make one creative loop reviewable.

Deliver a product specification, sourced model catalog, hardware and MIDI design, portable project format, plugin registry, candidate queue, local saves, piano roll, pads, keys, original vocal takes, MIDI 1 export, UMP packet export, and a compiled Mac development shell.

Exit when a musician can create a procedural arrangement, audition and accept it, change notes, make a playable sample, reopen the saved state, and export a valid MIDI file. AI features must identify their actual model. Physical MIDI input and microphone acceptance remain separate checks.

## Stage 1 A complete human recording loop

**Estimated 1–3 weeks after the foundation.** Make the musician's own input the strongest part of the product.

Add explicit arm and overdub controls, count-in, metronome, bar selection, clip offsets, audio import, waveform view, project archive import/export, session switching, durable undo, quantization choices, sustain pedal, note length editing, device selection, and recording latency measurement. Move project storage to a transaction-capable implementation when multiple related records require atomic commits.

Overnight continuations delivered complete project archives, session switching, recording arm, source-aware sustain, local audio import, failed-save recovery, exact note pitch/timing/velocity controls, source-preserving clip placement/trim, source-window transcription, and metronome/count-in for armed keys/pads. Auditions are isolated from recording. General Undo remains tab-local; microphone count-in, recording alignment/latency, waveform editing, device selection, and physical acceptance are still open. The remaining recording and editing work continues toward this stage's exit test.

Exit with an independently performed session: create project, play keys, record a vocal, undo a bad take, align clips, save, restart, restore all assets, and render a mix without lost notes or recordings. Include fault-injection recovery and a hardware loopback report.

## Stage 2 Actual local models

**Estimated 2–4 weeks; small adapter increments may land overnight.** Integrate one model per musical role and qualify it before broadening.

1. Basic Pitch or MuScriptor for singing and instrument transcription.
2. Aria MLX for piano continuation.
3. ACE-Step for song and audio editing.
4. Magenta RealTime 2 Small for a local performance experiment.
5. MiniMax Music 3 through a qualified external worker or local service.

Basic Pitch is integrated into the studio with real ONNX transcription. Aria now performs real MLX continuation through the studio candidate queue, with source-bar and response-budget controls, full-session review, separate audition, acceptance, and discard. Live performance and listening acceptance remain pending. [Aria evidence](ARIA-QUALIFICATION.md). Magenta Small now has a qualified bounded audio worker with text and optional note guidance, two actual renders, and a saved listening preview. General candidate controls, explicit clip placement, and offset-aware transcription are now implemented. Direct note/clip editing and source-window transcription are now implemented and browser verified. Live-streaming and hardware/listening acceptance remain open. [Magenta evidence](MAGENTA-QUALIFICATION.md).

Use separate environments, pinned model revisions, explicit download sizes, model manifests, cancellation, bounded resources, and machine-readable health checks. Do not make all runtimes share one Python environment.

Exit with original audio or notes in, real checkpoint inference, editable outputs, preserved source material, model provenance, a real cancellation test, and a saved A/B listening result. A declared endpoint or a mocked response is not model qualification.

## Stage 3 Playable instrument building

**Estimated 2–4 weeks.** Turn generated timbres into instruments worth playing.

Add sample mapping, multisample generation, root estimation, loop detection and editing, velocity layers, envelopes, round robins, and patch export. Qualify DDSP and RAVE as distinct performance-controlled neural instrument paths. Test Stable Audio 3 sample generation and compatible LoRA loading after license access is resolved.

Exit when a user can build a patch, play a two-octave phrase with dynamics, edit a bad mapping, save and reopen it, and export a portable instrument definition. Measure memory, response time, and pitch consistency.

## Stage 4 Human lead and an editable choir

**Estimated 3–6 weeks.** Build harmony from a real vocal with controllable musical parts.

Add lead phrase alignment, lyric transcription review, harmony constraints, SATB and contemporary voicings, per-voice score editing, a qualified singing engine, formant-preserving effects, and authorized voice conversion where useful. Include consonant alignment, breaths, dynamics, singer variation, and deterministic rendering of the chosen parts.

Exit with one human lead and at least three separately editable harmony voices. Preserve words, key, phrase timing, and the original lead; compare intelligibility and naturalness through listening. Export separate audio and note parts.

## Stage 5 Native audio and plugin hosting

**Estimated 4–8 weeks, with an earlier architectural spike.** Replace preview playback with a qualified realtime engine.

Evaluate the audio graph and native host framework before moving playback. Implement an Audio Unit host on Mac, plugin scanning in a recoverable process, parameter automation, latency compensation, crash handling, freeze/bounce, and project restoration. Add VST3 and CLAP according to user needs and SDK constraints.

Exit after a plugin crash does not corrupt the session, automation survives reload, recording stays aligned, and background model work does not cause additional dropouts under a documented rig and workload.

## Stage 6 Expressive performance and MIDI 2

**Estimated 2–5 weeks, overlapping native audio.** Complete the expressive performance path.

Add MIDI output, high-resolution capture and editing, per-note pressure/pitch/timbre mapping, MPE mapping, endpoint discovery, MIDI-CI capability inquiry, applicable profiles, controller maps, MIDI Clip File read/write, and conformance fixtures. Address same-pitch overlapping notes and note identity explicitly.

Exit with packet round trips, independent file validation, virtual endpoint tests, and an actual MIDI 2 device performance. Publish which dimensions and profiles are supported rather than a single undifferentiated "MIDI 2 supported" label.

## Stage 7 Windows and remote compute

**Estimated 3–6 weeks after contracts stabilize.** Keep portable music while changing platform services.

Build the Windows shell, audio devices and recording backend, MIDI Services integration, plugin scanning, application packaging, and GPU-worker connection. Preserve offline operation. A remote model worker receives only explicitly selected inputs and returns validated assets and provenance.

Exit with the same project opened, played, edited, rendered, and exported on Mac and Windows. Validate the installed Windows MIDI Services version and fallback behavior on the actual host.

## Stage 8 Advanced creative systems

Add alternate-take tournaments, musical constraint solvers, a controllable latent timbre garden, scene improvisation, evolving accompaniments, phrase memory, score-aware regeneration, lyric prosody coaching, expressive tuning systems, collaborative sessions, and performance-to-workflow capture.

Each experimental feature must leave a usable musical result in the ordinary project. An interesting model demo graduates only when the musician can steer it, undo it, save it, and use it in a song.
