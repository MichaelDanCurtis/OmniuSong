Finish OmniuSong as a complete, installable native music workstation written in Rust for Mac and Windows. Deliver Mac first, then Windows, using a shared architecture and portable project format.

Combine hands-on music making with multiple interchangeable AI models, expressive keyboards, pads, microphones, instruments, and plugins. The finished product must let a musician play, sing, compose, generate, arrange, edit, perform, mix, and export complete songs while retaining control over every creative decision. MIDI 2.0 and MPE are mandatory throughout the application.

Work exclusively in /Volumes/Main External/Development/OmniuSong and its task-specific worktrees. Preserve existing projects, original performances, useful implementation work, and unrelated edits.

MODEL STRATEGY AND DELEGATION

Use Astra at Extra high reasoning as the lead architect, coordinator, and integration reviewer where available. Use it for architecture, difficult Rust migration decisions, realtime audio correctness, expressive MIDI design, cross-platform boundaries, and complex failures.

Use GPT-6.1 Sol at Medium or Extra high reasoning for well-defined implementation tasks, tests, interface components, adapters, and documentation. Assign especially difficult subtasks to Astra when warranted. If the environment cannot select models independently for sub-agents, use the available configuration and report the limitation without stopping independent work.

You are explicitly authorized to use multiple sub-agents whenever useful. Delegate implementation, investigation, testing, and review; retain responsibility for the integrated result.

Normally use a coordinator plus up to three concurrent sub-agents, subject to available capacity. Use fewer when work is sequential or agents would compete for the same files, devices, or resources. Do not create parallel work merely to occupy slots.

Every delegated task must specify:

Its concrete deliverable and acceptance criteria.

The interfaces and architectural decisions it must follow.

The files or modules it owns.

Dependencies and work it must leave to other agents.

Required tests and verification artifacts.

Any resource or permission constraints.

Give agents separate worktrees where practical. Avoid overlapping file ownership. The coordinator owns shared contracts, dependency changes, migrations, and integration unless explicitly delegated. Integrate completed slices promptly and verify the combined application. A sub-agent’s completion report is evidence to inspect, not sufficient proof by itself.

Keep architecture, implementation, and review roles distinct for consequential changes. Arrange an independent agent review of persistence/recovery, realtime audio, plugin isolation, and MIDI 2.0/MPE before accepting their milestones.

REQUIRED ARCHITECTURE

Build the production application in Rust, including the desktop interface, realtime audio engine, recording, MIDI, editing operations, project storage, background jobs, model adapters, extension management, and plugin hosting.

Use a shared Rust workspace with portable core libraries and clearly separated Mac and Windows integrations. Deliver a native interface implemented in Rust. The installed application must manage its own lifecycle without a browser, JavaScript application runtime, Python service, or manually started development server.

Use Rust bindings for native operating-system services, accessibility, audio, MIDI, and plugin interfaces. Third-party plugins, model weights, and compatible native inference libraries may remain external dependencies; OmniuSong’s application logic, integration, and orchestration must be Rust.

Prefer Rust inference implementations or compatible native inference libraries. Any separate non-Rust service is an architectural exception requiring explicit approval before becoming a production dependency. Do not silently introduce one to complete an adapter.

Treat the existing prototype as a functional reference and migration source. Preserve its project data and useful behavior. Reproduce its qualified workflows in Rust; earlier prototype tests do not automatically qualify the replacement.

IMPLEMENTATION SEQUENCE

Follow these milestones in order. Parallelize only work whose prerequisites and shared interfaces are established. Each milestone ends with an integrated build, appropriate tests, UI verification where applicable, updated documentation, and explicit remaining acceptance gaps.

0. Establish the baseline and release contract.

The lead reviews the specification, roadmap, implementation plan, status, code, saved projects, and existing verification evidence.

Create a release acceptance matrix mapping every requirement to its implementation, test, platform, evidence, and status. Inventory existing assets and dependencies. Identify what will be ported, replaced, or retained as a reference.

Delegate bounded investigations into native Rust UI, audio/plugin architecture, and model-runtime compatibility. Each investigation must produce a concrete recommendation, constraints, and a small validation where needed. Resolve the architecture and record decisions before broad implementation.

Exit when the architecture, module boundaries, migration strategy, build commands, and acceptance criteria are documented sufficiently for agents to implement against them.

1. Build the native Rust application and shared contracts.

Establish the workspace, native application lifecycle, configuration, logging, errors, platform interfaces, and reproducible builds.

Define shared contracts for projects, assets, musical time, notes, expression, editing operations, audio commands, MIDI events, jobs, candidates, and extensions. Preserve unknown metadata and support explicit schema evolution.

Delegate independent scaffold components only after contracts are agreed.

Exit when the Mac application launches independently, presents its native interface, creates and reopens a project, and imports a representative existing project through a tested migration path.

2. Complete the reliable human recording foundation.

Run three coordinated workstreams:

Project agent: transactional storage, autosave, durable undo/redo, asset integrity, incremental recording recovery, migrations, and complete project archives.

Audio agent: native device selection, playback, synthesis, recording, monitoring, clocking, metronome, count-in, overdubbing, and recording compensation.

Interface agent: transport, track controls, arrangement, piano roll, waveform editing, recording-arm states, and clear recovery flows.

The lead integrates these into one musician-facing workflow.

Exit when a user can play, record, edit, undo, save, restart, restore an archive, and render audio without losing original material. Include interruption and fault-injection tests.

3. Complete MIDI 2.0, MPE, and expressive editing.

Delegate native MIDI transport, expression editing/routing, and independent protocol testing as separate tasks after agreeing their event contracts.

Implement MIDI 2.0 input/output, UMP handling, endpoint discovery, applicable MIDI-CI negotiation, high-resolution performance data, and supported per-note expression. Preserve original packets and timing alongside editable data.

Implement MPE input/output, zone configuration, channel allocation, per-note pitch, pressure, and timbre, sustain, overlapping notes, and reliable release behavior.

Provide expression lanes and route supported expression into built-in instruments and compatible plugins. Preserve expression through edits, undo/redo, saves, archives, and cross-platform transfers.

Implement appropriate MIDI file interchange, including standard MIDI 2 Clip Files, with independent validation. Clearly identify diagnostic UMP traces. Support ordinary MIDI 1 devices and document conversion losses and fallback behavior.

Exit only after separate MIDI 2.0 and MPE verification using packet fixtures, virtual endpoints, real controllers, compatible instruments, and audible playback. Include repeated/overlapping notes, sustain, disconnect/reconnect, and all-notes-off recovery. Mark unavailable hardware checks pending while continuing other work.

4. Deliver instruments and native plugin hosting.

Parallelize instrument development and plugin-host implementation against the stable audio and expression contracts.

Implement playable patches with root-note mapping, envelopes, loop editing, velocity layers, expression mappings, saved assets, and portable export. Demonstrate playing, correcting, reopening, and exporting an instrument.

Deliver Audio Unit hosting on Mac and VST3/CLAP hosting across applicable platforms. Include recoverable scanning, plugin state restoration, automation, latency compensation, freeze/bounce, and crash recovery.

Define stable extension boundaries without depending on Rust’s internal binary ABI.

Exit when instruments and plugins participate in the same saved musical session, retain supported expression and automation, and a plugin failure cannot corrupt the project.

5. Build the Rust AI substrate and qualify actual models.

First complete versioned adapters, manifests, parameter schemas, model/version pinning, installation management, isolated workers, cancellation, resource limits, progress, output validation, and restart recovery.

Then delegate independent adapters by musical role. Integrate and qualify each before expanding its scope. Inspect machine resources before running concurrent inference or downloading weights.

Qualify at least one working adapter for:

Lyric writing and revision.

Audio-to-MIDI transcription.

Symbolic composition and continuation.

Full-song generation.

Stem separation.

Instrument/sample creation.

Singing synthesis.

Interactive accompaniment.

Carry forward the useful capabilities demonstrated by Basic Pitch, Aria, and Magenta through qualified implementations satisfying the Rust architecture.

Support selected-region generation, continuation, variation, and regeneration where the model permits them. Demonstrate interchangeable providers for at least one role. Support compatible model modifications and sound packs through explicit compatibility declarations.

Exit for each adapter requires real inference, retained outputs, exact model/runtime identity, validated musical placement, cancellation/failure handling, preserved sources, and listening evidence. A placeholder, configured endpoint, or mocked response does not qualify an adapter.

6. Complete the integrated creative workflows.

Implement the remaining songbook, arrangement, instrument-building, and co-producer interactions.

Users must be able to select scope, lock material, compare alternatives, audition in context, edit results, and explicitly accept or discard candidates. Acceptance must be reversible. Clearly distinguish actual inference, procedural composition, conventional effects, and estimated transcription.

Complete the choir workflow: record a human lead, review melody and lyrics, create at least three separately editable harmony parts, and render separate voices. Provide controls for chords, ranges, timing, dynamics, variation, and stereo placement. Preserve the lead and export individual audio and note parts.

Any voice conversion or identity-based processing requires explicitly authorized material and a visible operation.

Exit when these capabilities work together in a complete song rather than isolated demonstrations.

7. Qualify and package the Mac release.

Complete accessibility, keyboard navigation, focus behavior, display scaling, onboarding, device changes, actionable errors, permission handling, and shutdown/recovery.

On a documented Mac rig, demonstrate under 10 ms software note response and 30 minutes of recording without dropouts. Measure physical input/output latency separately. Verify that bounded background inference introduces no additional audio dropouts.

Produce an installation package and test fresh installation, launch, model setup, project migration, updates, and uninstall behavior. An installed release must complete supported local workflows without development tools.

Exit when the Mac acceptance matrix passes, with any distribution credentials or external approvals explicitly identified.

8. Complete Windows parity and cross-platform acceptance.

Keep Windows contracts and build compatibility under consideration throughout development. After the Mac foundation is stable, qualify the actual Windows audio, MIDI, plugin, interface, recording, and installation paths.

Delegate Windows platform work against the established portable contracts. Preserve the shared project format and adapter behavior.

Test installation, devices, MIDI 2.0, MPE, plugins, recording, recovery, project exchange, playback, and rendering on an actual Windows system. Mac test results do not qualify Windows behavior.

Exit when the same project can be opened, edited, played, restored, and rendered on both platforms with its supported musical information intact.

FINAL ACCEPTANCE SESSION

Install the native Mac app. Create a complete song from a human keyboard/pad performance and recorded vocal. Revise lyrics, generate accompaniment, build a playable instrument, create three harmony voices, and use an audio plugin.

Record and edit expressive MIDI 2.0 and MPE performances using appropriate equipment. Verify audible expression and saved musical data.

Mix the song and export audio, stems, and supported MIDI formats. Restart the application, restore its complete archive, and verify original performances, edits, instruments, plugin state, automation, expression, and provenance.

Open the project in the installed Windows application, continue editing, play it, and render it. Retain reproducible verification artifacts and listening decisions.

OPERATING RULES AND COMPLETION

Continue autonomously through implementation, delegation, integration, meaningful tests, UI verification, and fixes. Make routine engineering decisions within this scope. Update status and acceptance evidence after each milestone.

Preserve human performances and unrelated edits. Do not modify Fuego, purchase anything, make paid calls, upload personal recordings, accept gated licenses, or publish releases without explicit authorization. Keep downloads bounded and runtimes isolated.

When an external decision blocks one requirement, finish independent work and identify the exact remaining decision. Hardware tests, listening judgments, actual inference, installers, and platform acceptance remain pending until performed.

Do not silently reduce requirements to declare success. Do not declare completion based solely on successful compilation, automated tests, or agent reports.

Declare completion only when the release acceptance matrix passes and the final demonstration is reproducible. Deliver the Rust source workspace, Mac and Windows installers, example song, verification evidence, user instructions, extension documentation, licensing/dependency inventory, and an accurate statement of supported capabilities and remaining limitations.
