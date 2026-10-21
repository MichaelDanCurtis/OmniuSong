# Rust migration delivery plan

October 20, 2026. The [full goal](GOAL.md) and [release acceptance matrix](RELEASE-ACCEPTANCE.md) supersede the original overnight-preview implementation plan. Scope remains the full native workstation on Mac and Windows, with real inference, plugins, MIDI 2.0 and MPE.

## Evidence before replacement

The existing Node/browser/Swift system remains a reference and a way to access saved music. Its source and ignored `data/` directory are not removed or overwritten. Native migration reads source projects/assets, validates them and creates independent native copies. Native evidence is separate from the 116-test preview suite and earlier Python inference receipts.

Initial read-only inventory: 19 project files, schema 1; 11 assets totaling 7,524,724 bytes. The inventory records exact hashes. Rust 1.98.1 and Cargo 1.98.1 are installed; the host reports 128 GiB RAM, 16 CPUs and roughly 655 GiB free on the project volume. These are resource observations, not performance qualification.

## Milestones and dependencies

| Milestone | Implementation | Exit evidence |
| --- | --- | --- |
| 0 Baseline/contracts | Requirement matrix; native UI/audio/MIDI/plugin/model investigations; architecture decisions | Linked sources, concrete module/API boundaries, migration fixtures and reproducible build commands |
| 1 Native shell | Shared Rust workspace/core; native UI; configuration/logging/lifecycle; independent project store and legacy import | Actual Mac native launch; create/save/reopen; representative imported session and originals/metadata unchanged |
| 2 Recording foundation | Transactional storage/undo/recovery; native audio/device/clock/capture; native editors/transport | Integrated play/record/edit/save/restart/restore/render, fault injection and actual input/alignment gates |
| 3 Expressive MIDI | Separate project data, UMP codecs, native transport, discovery/CI, standard Clip Files, MPE, lanes/routing | Independent fixtures, virtual and physical performance, audible expression, documented fallback |
| 4 Instruments/plugins | Mapped instruments; supervised AU/VST3/CLAP scanning/hosting; automation/compensation/freeze/bounce | Saved integrated session, actual plugin sound/state, isolated crash and latency fixtures |
| 5 AI substrate/adapters | Rust supervisor and versioned contracts first; then independently qualified role adapters | Actual native inference, outputs, identities, cancellation/recovery/listening for every required role |
| 6 Creative workflows | Songbook/co-producer, editable choir with real human lead and three parts | Complete song with locks, reversible candidates and per-voice scores/renders |
| 7 Mac release | Accessibility, hardware/latency/stability/load tests, installation/update/uninstall, docs | Mac matrix passes, actual installed workflow, any signing/distribution decision explicit |
| 8 Windows parity | Shared contracts retained; actual platform backends, devices/plugins/inference/installers | Actual installed Windows workflow and same song transferred/edited/rendered on both platforms |

Windows API/build considerations begin at architecture time. Passing Mac tests does not close Windows acceptance. MIDI/software/hardware/listening/packaging evidence remains distinct throughout.

## Coordination

The coordinator owns shared contracts, workspace/dependency changes, migration semantics, integration and acceptance evidence. Up to three agents work on non-overlapping tasks. Initial tasks own only their individual research documents and an isolated UI experiment. Investigation must lead to bounded implementation; no parallel scaffold guesses before interfaces are agreed.

Use Astra Extra high for difficult architecture and independent integration review where available, Sol Medium/Extra high for bounded implementation and tests. Models can be selected for sub-agents in this environment. Any coordinator model limitation remains visible rather than stopping useful work.

After milestone 1 contracts, the recording milestone separates project-store, audio-engine and UI owners. MIDI transport, expression editing and independent protocol tests separate after their contract is stable. Instruments and plugin hosts may then proceed independently. Model adapters parallelize only after the worker/candidate protocol exists, and actual inference/downloads remain resource-bounded.

Every assignment includes deliverable, owned files, interface dependencies, excluded work, tests and resource/permission limits. A reviewed agent report is not a substitute for integrated tests, actual native UI verification, listening, hardware, or platform checks.

Use milestone IDs 0–8 consistently with GOAL.md. Before starting a work packet, name its matrix rows, prerequisite contracts, one owner, exact file ownership and exit evidence. The coordinator resolves shared changes before agents implement them. Do not close a milestone while its exit checks are pending; available independent work may continue with that gap recorded.

| Stage | Delegation and ownership | Integration dependency |
| --- | --- | --- |
| 0–1 | UI owner: app and isolated UI probe. Audio/MIDI owner: device, plugin and realtime investigation/contracts. Model owner: runtime investigation and independent migration review. Coordinator: workspace, core, store, decisions and evidence, unless explicitly reassigned. | Adopt boundaries before broad implementation; native create/import/reopen must be demonstrated. |
| 2 | Store owner: durable projects, original chunks, recovery and archives. Audio owner: engine, platform devices, clock and capture. UI owner: app transport/editors and recovery flows. Core contracts and workspace manifests stay with the coordinator. | Agree chunk formats, command/acknowledgement semantics and ownership before parallel edits. Integrate one playable, recoverable recording workflow. |
| 3 | Native MIDI owner: codecs, endpoint transport and capability negotiation. Expression owner: MPE allocation, editing and instrument routing. Independent reviewer: protocol fixtures and release/reconnect failures. | Stable audio/capture clocks and note identities; real endpoint and audible evidence remain separate. |
| 4 | Instrument owner: playable patches and asset mapping. Plugin owner: supervised scan/processing, state, automation and compensation. Reviewer: isolated crashes, recovery and expression interoperability. | Stable audio/expression contracts and verified persistence. |
| 5 | Substrate owner first; then one owner per adapter role, in bounded batches of at most three. An independent agent reviews each adapter's runtime identity and retained inference outputs. | Worker/candidate protocol, installation/resource limits and source-safe acceptance exist before adapters parallelize. |
| 6–8 | Workflow, release/platform and independent acceptance owners take separate packets. Windows source/build preparation can proceed earlier; Windows runtime acceptance needs an actual Windows system. | Choir and complete-song workflows depend on qualified instruments/models. Installers and the final cross-platform session require integrated features, not isolated demos. |

Every handoff names the changed files, commands and artifacts, unresolved defects and exact next consumer. Agent work stays reviewable locally; the coordinator reruns meaningful integrated checks after shared changes and keeps human/hardware/permission decisions visible.

## Required boundaries

- The durable document owns music; the audio thread reads prepared immutable/command data, never project JSON or the database.
- Musical ticks, audio frames and native clock timestamps are distinct types; calibration is explicit.
- Internal note IDs, raw MIDI source/address, captured packets and edited expression are distinct. No MIDI 2 device note-ID guarantee is invented.
- Device adapters and codec logic are separate; UMP packet support does not imply native delivery, MIDI-CI or standard Clip Files.
- Project and asset integrity are checked before publishing an imported session. Unknown namespaced/model metadata survives round trips.
- Native application state and recovery use a separate store during migration; existing originals remain read-only.
- Inference and plugin scanning/processing have supervised boundaries. Stable external protocols/C ABIs do not rely on Rust compiler ABI.
- No production Python/Node service is grandfathered in from the prototype. Native inference libraries are allowed; non-Rust services need explicit approval.

The final acceptance session and every release deliverable remain mandatory even when a platform, device, model license, or listening decision is temporarily unavailable.

## Current reproducible commands

```sh
cargo fmt --all -- --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
bash scripts/build-native-mac.sh
```

Open `dist/OmniuSong Native.app` for native UI qualification. No prototype server is required. A new application identity may need the user's explicit macOS external-drive access decision. The initial native loading UI is created before filesystem initialization and stays responsive while a worker waits; it does not silently retry or change permissions.

Read-only migration qualification requires a fresh destination directory to avoid replacing any existing native work:

```sh
cargo run --locked -p omniusong-store --example qualify_legacy -- data .runtime/rust-migration/new-qualification docs/verification/rust-native-migration.json
```

The exact objective is retained in GOAL.md; the matrix records implementation/test locations, Mac/Windows states and required/actual evidence per requirement. The [dependency inventory](NATIVE-DEPENDENCIES.md) and independent review documents are part of the baseline, not a substitute for future device, inference, installer or listening evidence.

The [arrangement source gate](verification/rust-native-arrangement-tests.json) passes **562 entries**, including 173 app cases, formatting, Clippy, workspace build and the **09789029…** Mac development package/signature. Its [bounded actual walkthrough](verification/rust-native-arrangement-ui.json) now passes guarded move/snap/draft/history/archive/restart/output checks while preserving all earlier projects/originals. The [contract](contracts/NATIVE-ARRANGEMENT.md) keeps display geometry, typed placement and the existing draft/save/history boundary separate. The first [performance tap](contracts/NATIVE-PERFORMANCE-TAP.md) passes a [592-entry source/software gate](verification/rust-native-performance-tap.json) and actual bit-exact logical audio proof. It remains library-only and is not packaged into 0978. The [636-entry writer qualification](verification/rust-native-performance-writer.json) now verifies atomic incremental originals, semantic checkpoint recovery and isolated complete archive restoration. Worker/UI recording, healthy normal Finish, musician-facing attachment and explicit derived editing remain required; the installed 0978 bundle is unchanged.

Earlier [integrated clip/editor/note/audio/storage source evidence](verification/rust-native-clip-editor-tests.json) has **469 passing entries**, zero failures, two intentionally ignored subprocess entrypoints, clean formatting/Clippy and the rebuilt signed **216d8e1c…** development bundle. The [bounded actual clip/edit/render/archive/restart/output walkthrough](verification/rust-native-clip-editor-ui.json) passes separately. Older 394/425-entry source gates and the 85ef note UI keep their original boundaries. Seven crates include the native audio process, fixed callback transfers and retry-safe take publication. The [software-bus qualification](verification/rust-native-audio-software-bus-final.json) preserves 108,000 stereo frames exactly without opening native input. Native input remains unexercised under software-loopback-only authorization.

The [earlier actual Mac UI](verification/rust-native-disconnect-ui.json) completed two backup/copy generations, paired edits/history, malformed-backup rejection and process restart/reopen. That evidence belongs to its 191-entry binary. The [actual cb64 output/recovery/archive walkthrough](verification/rust-native-capture-recovery-ui.json) now passes device selection, 48-kHz saved-session playback/Stop, verified-prefix recovery, dirty-save attachment, Undo/Redo and restoration. Its 270,336-byte archive and damaged original rows pass [preservation checks](verification/rust-native-output-preservation.json). The 358… live bundle subsequently completed output/control/restart/source reopening and exposed a piano focus defect. The corrected 2dd… bundle completed its own overlap/sustain/accessible keys/pads/cleanup/Stop/process restart/source-reopening walkthrough. It does not inherit prior-binary acceptance; full text/IME/window-focus-loss/scaling remains separate. The [preservation audit](verification/rust-native-audio-preservation.json) confirms all six prior native sessions and all 30 prototype files remain exact. Earlier migration/archive/copy receipts remain historical evidence at their named source boundaries.

Full milestone 2 remains incomplete. The bounded [current exact-binary UI/output/reopen gate](verification/rust-native-live-ui.json) now passes. Live keys/pads now pass the [implemented contract/review](contracts/NATIVE-LIVE-AUDITION.md), independent tests and [synthetic render/archive proof](verification/rust-native-live-bus.json); the prior-binary output/control walkthrough is retained and the narrow focus fix passes its own current-binary walkthrough. The [finite note backend](contracts/NATIVE-NOTE-EDITING.md) now passes [independent review](reviews/RUST-NOTE-EDITOR-CONTRACT.md) and [isolated storage/archive qualification](verification/rust-native-note-editing.json). Exact-identity note controls complete their [85ef bounded actual qualification](verification/rust-native-note-editor-ui.json). The later 216d build passes 112 app headless cases and its own [actual clip qualification](verification/rust-native-clip-editor-ui.json), preserving all earlier projects and source objects. The bounded exact-binary arrangement walkthrough is complete; finish saved performance capture and transport/clock/buffer controls next; waveform dragging remains separate. The [retained-stage workflow](contracts/NATIVE-CAPTURE-RECOVERY.md) now implements bounded discovery, unchanged-byte inspection, explicit prefix recovery and separate attachment; keep-local and typed-conflict choices preserve intents without trapping session navigation. The [isolated recovery/archive qualification](verification/rust-native-capture-recovery.json) and [independent review](reviews/RUST-CAPTURE-RECOVERY-INITIAL.md) cover that bounded workflow. General set-aside and combined unassigned-source export remain pending. Monitoring, overdub, metronome/count-in, compensation and actual input/alignment remain pending. Playback has explicit supported-format/rate rejection and 120-second/64-MiB preparation bounds; no resampling. No unit/software-bus evidence qualifies physical listening/latency, thirty-minute recording, expressive MIDI, plugins, inference, installers or Windows.

## Drive-disconnect recovery and bounded follow-up

The [incident record](DISCONNECT-RECOVERY.md), [candidate qualification](verification/rust-native-disconnect-recovery.json), [reversible promotion](verification/rust-native-disconnect-promotion.json) and [independent recovery review](reviews/RUST-DISCONNECT-RECOVERY-INITIAL.md) retain separate evidence. All four current project documents survived the shortened development catalog. Three complete projects were restored exactly; the fourth's surviving revision-8 content became a fresh revision-0 fork with source JSON, complete earlier backup and recovery evidence embedded. One history row and three old operation receipts are missing. No historical admissions were fabricated, and the raw damaged store remains separately retained. The physical cause is undetermined.

The worker now retires its connection generation after delivering a surfaced typed SQLite `IOERR`, `CORRUPT` or `NOTADB` failure. The UI retains its draft and safe durable intent; reconnect and replay are separate explicit actions. Existing-only reconnect uses `NativeStore::open_existing` without SQLite `CREATE` or directory provisioning. This is bounded admission behavior, not general repair or removable-drive resilience. Late archive wrappers, stringified/per-project errors, OS I/O stall deadlines, persistent volume/catalog enrollment, general recovery UI, independent checkpoints and physical power-loss qualification remain open. H07/H08 retain actual rebuilt UI confirmation for two source-fixed issues: warning contrast in the light theme and stale background-work text after a completed ordinary error. The current headless source gate covers their corrected state; complete accessibility/focus/scaling remains pending. No audio/MIDI/inference/plugin/Windows acceptance follows from this incident or its recovery.


The retained synthetic archive qualifier is Rust and requires a fresh fixture directory:

```sh
cargo run --locked -p omniusong-store --example qualify_native_archive -- .runtime/rust-migration/new-archive-qualification docs/verification/new-native-archive.json
```

It streams 33 MiB PCM and 5,120 raw events, removes the current take reference through Undo/branching, exports the operation-result closure, restores into another store and compares exact originals and old receipt replay. It does not qualify microphone capture or a standard MIDI format. The separately named `prepare_native_ui_fixture` example creates a tiny synthetic recording project through the checked store API for actual native UI inspection; it never replaces existing projects.

The retained archive-copy qualifier reads an authentic earlier-format archive without modifying it and requires a fresh fixture directory:

```sh
cargo run --locked -p omniusong-store --example qualify_archive_copy -- OLD_V1_ARCHIVE .runtime/rust-migration/new-copy-qualification docs/verification/new-native-copy.json
```
