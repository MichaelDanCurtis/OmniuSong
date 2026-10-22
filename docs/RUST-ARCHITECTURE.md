# Native Rust architecture and decisions

October 20, 2026. Scope: [full goal](GOAL.md); gates: [release acceptance](RELEASE-ACCEPTANCE.md); sequence: [migration plan](RUST-MIGRATION.md). Research is linked below and is not runtime qualification.

## Decision 1: native Rust UI

Use **egui/eframe 0.37.0**, a Rust-rendered interface on native winit windows, with wgpu Metal on Mac and DX12 on Windows. Enable the native AccessKit adapter and retain an accessible keyboard/note-list alternative to custom painting. This is a native application; no webview/server is part of the new path.

The [UI investigation](research/RUST-UI.md) checks exact tagged APIs, alternatives, versions and license terms. Its independent headless spike demonstrates real egui rendering/input/accessible-tree behavior, not OS assistive-technology or physical device acceptance. Mac launch, keyboard/focus/scale and actual VoiceOver must be tested separately; Windows remains its own gate.

The [October 22 NSH evaluation](research/RUST-UI.md#nsh-evaluation) shortlists a Rust-only AppKit/Win32 alternative for an isolated prototype. Source inspection found key-release/focus, accessibility-action and guarded-close integration gaps. No framework migration is selected; keep storage/audio/MIDI boundaries independent and require native workflow evidence before replacement.

## Decision 2: lossless migration and transaction-capable storage

Initially retain schema-1 musical documents through a validated `ProjectDocument` boundary. Domain projections expose distinct musical ticks, signed timeline sample frames, unsigned source-frame offsets, velocity and expression types. Unknown metadata and all source note/clip/capture fields remain in their original JSON values; a projection is never used to serialize away unfamiliar fields. JSON numbers in opaque metadata retain arbitrary precision. A future schema introduces tempo maps/sample scheduling through an explicit migration, not silent reinterpretation of ticks.

Use **rusqlite 0.40.2 with bundled SQLite** for the independent native store. Projects, content-addressed source assets, exact original import JSON and edit-history entries publish in one transaction. This avoids the prototype's cross-file publication split. The initial store uses SQLite's rollback journal and `synchronous=FULL`, bounded busy timeout and immediate write transactions with expected-revision checks. No database work belongs on the audio thread.

Initial native data lives under `.runtime/native-studio/` for development, or a caller-selected `--data-dir`. Source `data/projects/` and `data/assets/` are read-only imports. Each import gets a fresh native project identity; tracks, notes, expressions, raw capture and unfamiliar metadata survive. Source file bytes and their hash are retained independently; any existing `nativeImport` value remains in the new receipt's `previousImport`. All schema-owned audio references must be present and checksum-valid before the session becomes visible. Reference discovery follows explicit schema-1 track and provenance-input paths; it does not infer local files from arbitrary provider fields named `assetId`. Failed imports leave no new visible project. Durable undo/redo snapshots and their cursor commit with the project revision; a new edit after undo explicitly abandons the redo branch. A damaged individual project remains visible with a recovery diagnostic while healthy sessions can still be listed and opened.

SQLite is a native library, allowed by the goal. Its [atomic commit design](https://www.sqlite.org/atomiccommit.html) and the [rusqlite transaction interface](https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html) establish the mechanisms; OmniuSong still needs its own process-kill, corrupt-media, concurrent-writer and full recording-fragment qualification. A rolled-back test transaction is not a power-loss result.

## Initial crate boundaries

| Crate | Owns | Must not own |
| --- | --- | --- |
| `omniusong-core` | Validated portable musical documents, metadata retention, domain types and explicit musical operations | UI, audio devices, database connections, model inference |
| `omniusong-store` | Transactional snapshots/history, integrity checks, exact import receipts and source assets | Audio callback, destructive writes to prototype sessions |
| `omniusong-app` | Native views, accessible controls, editor drafts and presentation of commands/results | Model/audio work in paint callbacks, direct source recording mutation |
| `omniusong-recording` | Separate per-take journals, bounded immutable chunks, durable prefixes, recovery/index proofs and sealing | Device callback, automatic recording resume, opaque schema-1 publication |
| `omniusong-performance` | Device-free owned local-performance types/codec, shared checkpoint verification and authenticated procedural note projection | Devices, tap/ring lifecycle, native normal-seal authority, project commits or UI |
| `omniusong-ipc` | Bounded non-realtime framing/admission and private Unix child control connections | Audio callback, project commits, unqualified Windows transport or plugin sandbox claims |
| `omniusong-session` | Capacity-one background storage commands, durable retry identity, session/archive operations and sealed-capture attachment | Audio callback, GUI paint, automatic uncertain-operation replay |
| `omniusong-engine` | Private native-audio worker, bounded saved-session preparation, CoreAudio output, solo-input adapter and incremental capture transfer | Project publication, model/plugin work or filesystem/IPC in sample callbacks |

The first [native audio boundary](contracts/NATIVE-AUDIO-ENGINE.md) uses CPAL 0.18.2, rtrb 0.4.0 and a narrowly patched, provenance-retained coreaudio-rs 0.14.2. General streaming graphs/live voices, separate device/MIDI adapters, `midi` (UMP/MPE/CI/Clip File semantics), `jobs`, `models`, `plugins` and platform packaging remain planned boundaries. Add a crate when it owns working behavior and tests, rather than empty scaffolding.

The [performance projection boundary](contracts/NATIVE-PERFORMANCE-PROJECTION.md) places shared interpretation below the engine so later store/archive publication can recompute against the same held original without a store-to-engine cycle. Existing engine paths reexport the same owned types and verifier. Runtime queues, callbacks and private native seal permits stay in the engine. Read-only projection is separate from schema-3 project ownership, atomic publication and native note controls; those subsequent stages must not accept a caller-produced digest as source authority.

## Decision 3: audio and expressive events remain distinct from control

The UI/store/inference side owns editable state. The realtime callback consumes prepared, bounded commands/data without waiting on a project/database mutex, allocation, filesystem, network or inference. Disk recording uses a separate bounded transfer and incremental durable journal with a visible committed-loss boundary. Device timestamps, musical ticks and audio sample frames require an explicit calibration; retained host timestamps are not already an audio clock.

Internal voice identity distinguishes repeated/overlapping notes. Source/group/channel addresses, original packets, measured expression, edited expression and model-estimated curves are preserved separately. Do not invent a universal MIDI 2 protocol note identifier. MIDI 2 project values, UMP encoding, native endpoint delivery, MIDI-CI, standard Clip Files, MPE, and physical/audible acceptance have separate matrix rows.

The [audio/MIDI/plugin investigation](research/RUST-AUDIO-MIDI-PLUGINS.md) records concrete backend and ABI choices plus platform/version/license gaps. In particular, Windows MIDI Services deployment/API availability must be runtime-qualified on the actual supported Windows version rather than assumed from an SDK preview.

## Decision 4: Rust-owned model and plugin supervision

Use typed, versioned external protocols and narrow C ABIs for compatible native libraries; do not use Rust's internal compiler ABI as a third-party extension contract. Plugin scan/host failures and model workers have recoverable supervised boundaries. Audio processing and offline inference have separate budgets and scheduling. Adapter installation, checkpoint integrity, load readiness and completed inference are separate states.

First native inference target: Basic Pitch's existing small ONNX graph, with Rust preprocessing/decoding and a native ONNX runtime. Compare activations/decoded results with frozen reference fixtures. Next qualify the official native Magenta core through a Rust-owned boundary. The [model investigation](research/RUST-MODELS.md) covers every required role and documents actual port/build/access gaps; no Python worker is accepted as native production inference. It also identifies a placeholder native audio-prompt path that must not be advertised as genuine conditioning.

Native libraries are allowed; any separate non-Rust service requires explicit user approval. Models and voicebanks retain their own terms. No license acceptance, credentials, paid calls or personal-recording upload is implied by a manifest or by research.

## Decision 5: adopted versioned process and long-recording contracts

Adopt [NATIVE-PROTOCOLS revision 1](contracts/NATIVE-PROTOCOLS.md) and the checked `omniusong-core::contracts` types. They define separate versioned audio-control/event, capture, edit, job/candidate and worker-handshake envelopes; explicit byte/address/value limits; clock domains/epochs/anchors; and fixed-capacity Copy-only callback data. JSON and unfamiliar metadata never become callback payloads. The source definitions and their tests establish checked contracts, not running IPC or audio behavior.

The production topology is a native Rust application supervising a Rust engine process, isolated per-instance plugin workers/disposable scanners and bounded model workers. The engine has a control/preparation side and bounded preallocated realtime queues; UI lifetime does not own the device callback. Mac uses private local sockets and Windows private named pipes for control. Actual authentication/ACLs, shared memory, worker isolation, callback scheduling and overload behavior must be implemented and independently tested before their matrix rows pass. Subprocess placement alone is not a secure sandbox.

Long recordings adopt immutable bounded chunks in a separate per-take SQLite journal, a checked durable prefix and explicit gaps. A sealed manifest publishes into the main project transaction after the source is durable. Recovery retains interrupted originals and does not invent successful samples for an unknown/uncommitted tail. Archives must include all referenced chunks/manifests and source/undo/candidate relationships using a consistent read; copying an active SQLite file is insufficient. The initial 30 MiB source-asset/1.9 MB project/4,096 embedded-event limits apply only to the schema-1 bridge and must not limit production recording. See the protocol document for selected queue/chunk capacities and required fault/device tests.

These decisions give the recording, device, interface and worker agents concrete boundaries for milestone 2. [Recording storage](contracts/NATIVE-RECORDING.md), [private Unix IPC](contracts/NATIVE-IPC.md) and [idempotent edit admission](contracts/NATIVE-EDITS.md) have checked implementations. Musical [schema 2](contracts/NATIVE-TAKES.md) now adds typed recording tracks; existing notes/audio metadata remains opaque. The main database's additive schema 3 registers [owned sealed take files](contracts/NATIVE-TAKE-STORE.md). Durable ingestion precedes a separate atomic project/history/operation publication. Physical file identity and semantic manifest identity are distinct, and all distinct sources retained by a resulting edit must verify before it commits.

[Native archives](contracts/NATIVE-ARCHIVE.md) retain the complete reference closure of current projects, both history sides, all operation results and exact imports. Large native files are streamed in bounded chunks, including recovered originals' unselected tails. Identity-preserving restore validates staged objects before one metadata publication transaction; restore-as-copy remains a separate required design. A [synthetic qualification](verification/rust-native-archive-roundtrip.json) restores 33 MiB audio and 5,120 raw events retained only by old operation receipts. These storage boundaries do not provide native capture, playback, measured timing or elapsed device recording. Full-file source verification is blocking storage work; background store/action integration is required before connecting long takes to interactive editing.

## Review and proof

Every milestone includes integrated build/tests, actual UI inspection for UI changes and updated matrix evidence. Persistence, realtime audio, plugin isolation and expressive MIDI each receive independent agent review before acceptance. Review findings must be fixed or left as visible open acceptance work.

The initial native shell is allowed to expose incomplete controls as clearly unavailable while infrastructure is being built. It must not pretend to record, play through a native engine, host plugins, or perform model inference before those paths work. This is an implementation stage, not a reduced release scope.


## Retained-stage review and recovery boundary

The [capture recovery contract](contracts/NATIVE-CAPTURE-RECOVERY.md) adds a read-only, exclusively owned retained-journal view, bounded stage discovery and explicit checked prefix sealing. The owner guard transfers directly into the recovery writer. Full physical and normalized original-row fingerprints bind review, checked transitions and retry, including damaged/unselected rows and opaque fields. Original chunks are retained; unknown tails remain unknown. Project attachment stays a separate stable-ID/revision transaction. The native interface exposes explicit review/recover/attach, keep-local and typed conflict release, with no automatic capture, recovery, replay or rebase. [Independent review](reviews/RUST-CAPTURE-RECOVERY-INITIAL.md) and [278-entry source evidence](verification/rust-native-capture-recovery-tests.json) qualify the bounded software workflow; actual current UI, device, stalled-IO, physical repair and release gates remain separate.
