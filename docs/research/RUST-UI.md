# Native Rust UI decision

Researched October 20, 2026 against the existing OmniuSong product specification,
implementation plan, Swift shell and portable studio. This decision replaces the
development browser shell for the native application; it does not establish audio,
MIDI, model or plugin readiness.

The [October 22 NSH evaluation](#nsh-evaluation) adds a source-reviewed candidate
and a bounded comparison plan. It does not replace the current egui decision.

## Decision

Use **egui and eframe 0.37.0**, pinned exactly, with the **wgpu 30.0.1** renderer
and **AccessKit** platform adapters. Enable Metal on Mac and DX12 on Windows.
Keep domain documents, persistence, audio, device input and jobs independent of
the UI toolkit. The application is a native Rust executable without a browser,
JavaScript server or Python requirement for its application shell.

The decisive fit is OmniuSong's custom, dense musical workspace: a piano roll,
waveforms, expression, sixteen pads and transport controls. egui's custom mesh
painting and explicit widget interaction can share the same musical geometry;
ordinary controls already contribute accessible roles. Its permissive licenses
avoid a UI toolkit royalty dependency. This is a recommendation, supported by the
bounded proof below, rather than a claim that egui supplies a finished DAW or
complete accessibility. The [tagged README](https://github.com/emilk/egui/blob/0.37.0/README.md)
also explicitly documents changing APIs and the absence of a native widget look.

## Options checked

Versions were checked against the official crates.io API, not search snippets.
All options below compile native applications; none requires a webview for its
native path.

| Choice | Current stable / license | Fit and deciding tradeoff |
| --- | --- | --- |
| **egui / eframe** | **0.37.0**, MIT OR Apache-2.0, Rust 1.95 minimum | Recommended. Straightforward custom note/waveform drawing, native Windows/Mac integration, AccessKit roles/actions, and a small testable surface. Immediate layout requires deliberate culling, repaint discipline and controlled upgrades. |
| **Iced** | **0.14.0**, MIT, Rust 1.88 minimum | Strong typed message/update architecture and custom `Canvas`; attractive for conventional retained application state. Released 0.14.0 does not provide the required AccessKit integration; upstream accessibility PR remains a draft. Do not make accessible music editing depend on that unmerged work. |
| **Slint** | **1.18.1**, GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0; Rust 1.92 minimum | Viable second choice with declarative components, native backends and an accessibility feature. A separate `.slint` language and bespoke dense music surfaces add integration work. Its free desktop distribution option includes specific attribution and API exposure conditions; no license was accepted or selected here. |
| **Xilem / Masonry** | **0.4.0**, Apache-2.0, Rust 1.88 minimum | Promising retained widget tree, Vello/wgpu and AccessKit foundation. Upstream explicitly describes the architecture as experimental. Additional framework maturation is avoidable project risk for the first full workstation. |
| **GPUI** | **0.2.2** published crate, Apache-2.0 | Excellent custom GPU drawing and editor precedent. Upstream source now describes native Windows/Mac support and a newer platform crate split. This moving architecture and the gap between published crate and current examples add maintenance cost. Do not repeat outdated claims that current GPUI has no accessibility: upstream now has an accessibility guide and ongoing AccessKit work. |

Primary comparison evidence: [Iced 0.14 manifest](https://github.com/iced-rs/iced/blob/0.14.0/Cargo.toml),
[Iced accessibility PR](https://github.com/iced-rs/iced/pull/1849),
[Slint 1.18.1 manifest](https://github.com/slint-ui/slint/blob/v1.18.1/api/rs/slint/Cargo.toml),
[Slint royalty-free 2.0 terms](https://github.com/slint-ui/slint/blob/v1.18.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md),
[Xilem 0.4 README](https://github.com/linebender/xilem/blob/v0.4.0/README.md),
[GPUI upstream README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md).
Registry records: [egui](https://crates.io/api/v1/crates/egui),
[eframe](https://crates.io/api/v1/crates/eframe), [Iced](https://crates.io/api/v1/crates/iced),
[Slint](https://crates.io/api/v1/crates/slint), [Xilem](https://crates.io/api/v1/crates/xilem),
[GPUI](https://crates.io/api/v1/crates/gpui).

## Concrete first application

The initial native shell should provide real session creation, listing, reopening,
read-only legacy import, explicit rename/tempo editing, save/discard and durable
undo/redo. It should render actual tracks and note timing from opened documents,
and show original recording and candidate provenance. Playback, recording,
controller connection and model generation remain disabled until their native
implementations are connected and independently verified.

Use three independent layers initially: `omniusong-core` owns the portable
document and musical operations; `omniusong-store` owns atomic durable storage,
assets and history; `omniusong-app` owns view state and command presentation.
Extend with engine/device/job crates behind typed interfaces. UI state includes
selected track/note, zoom, scroll, editor drafts and dialogs. It does not own an
audio clock or mutate source audio as a rendering side effect.

The studio layout is transport/status above a track and arrangement workspace,
an inspector beside the selected piano roll or waveform, pads/keys below, and a
compact candidate panel. Persist UI preferences separately from songs. Keep
audition, accept and discard distinct. Locked originals and generated candidates
need visible, textual status independent of color.

For exact note editing, store ticks, durations, IDs and 16-bit velocity in domain
types. Convert to `f32` only for the current viewport. Pan/zoom and hit testing use
the same transform. Cull invisible notes before creating widgets and clip their
paint to the roll. Use stable IDs from document identity, track ID and note ID,
not array positions. Draw a selected note inspector and a keyboard-accessible
note list; repeated notes at the same pitch/time must remain separately reachable.
Large projects should use a temporal index rather than scanning every note every
frame. The spike demonstrates culling, not this future index.

For waveforms, decode and compute per-channel min/max pyramid summaries on a
worker, keyed by asset checksum. Draw only the visible resolution. Show original
extent, selected trim and fades separately; edit handles produce reversible
project commands. Never read/decode full source audio during a paint pass. Reject
stale worker results using asset and selection generations. Preserve the legacy
waveform's distinction between saved state and unsaved preview.

## Current API and build setup

Use the pinned native dependencies below; retain a Cargo lockfile:

```toml
eframe = { version = "=0.37.0", default-features = false, features = [
  "accesskit", "default_fonts", "system_fonts", "clipboard", "wgpu_no_default_features"
] }
wgpu = { version = "=30.0.1", default-features = false, features = ["std", "metal", "dx12"] }
```

The exact 0.37 API uses `App::ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)`;
`App::logic(&mut self, ctx: &Context, frame: &mut Frame)` is optional and must not
paint. `CentralPanel::default().show(ui, ...)` wraps the supplied root UI. Launch
with `eframe::run_native`, `NativeOptions` and a creator returning
`Ok(Box::new(app))`. The [tagged App source](https://github.com/emilk/egui/blob/0.37.0/crates/eframe/src/epi.rs),
[hello-world example](https://github.com/emilk/egui/blob/0.37.0/examples/hello_world/src/main.rs)
and [feature definitions](https://github.com/emilk/egui/blob/0.37.0/crates/eframe/Cargo.toml)
were checked directly.

Important 0.37 details: standalone `egui` has no default fonts and no `accesskit`
feature; AccessKit types are mandatory and `default_fonts` is opt-in. eframe's
`accesskit` feature enables the native adapter. Use `Id::unique` for globally
stable IDs and `IdSalt` for local salts. Headless frames use `Context::run_ui`.
Panel APIs are unified as `Panel::top/bottom/left/right`, with `default_size`
and `min_size`; older `TopBottomPanel` and `SidePanel` examples do not compile.
Unused texture deltas must be explicitly cleared in headless tests; a real
renderer uploads them. Initial fonts resolution hit a stale Cargo sparse index;
`cargo update --manifest-path ...` refreshed it and successfully resolved the
published package. This was a local cache issue, not a missing release.

The eframe source pins compatible winit 0.30.13 and AccessKit 0.25.x. The proof
resolved AccessKit 0.25.1. Do not add a second incompatible winit/AccessKit version
to access native handles. Start with eframe's window lifecycle and use its window
handle access for narrow platform integration. Native menus and file dialogs can
use [muda 0.21.2](https://crates.io/crates/muda/0.21.2) (MIT OR Apache-2.0) and
[rfd 0.17.2](https://crates.io/crates/rfd/0.17.2) (MIT); they are optional,
separately qualified integrations, not bundled DAW features.

## Input, device lifecycle and scheduling

Use logical keys for command shortcuts and configurable physical keys for the
computer piano. Respect text/IME focus, consume most specific shortcuts first,
ignore repeat Note On, and always process releases for already-held notes even
if text gains focus. Focus loss cancels only computer/pointer voices; physical
MIDI can continue according to an explicit background-performance preference.
Emergency all-notes-off must remain accessible. Preserve source/group/channel,
same-pitch voice identity, sustain and release velocity from the current input
contract. [Current input events](https://docs.rs/egui/0.37.0/egui/enum.Event.html)
carry physical and logical keys, release state, repeats and focus events.

USB MIDI and audio discovery, disconnection, timestamp conversion, stream errors,
sample-rate changes and recovery belong to the engine/device layer, not winit.
CoreMIDI UMP remains a native Mac adapter concern; Windows has its own device
adapter. A disconnected device releases its own active voices without stopping
other sources. macOS sleep/wake and default device changes require actual
platform observation and re-enumeration; a window `resumed` event alone is not
proof of audio recovery. Display/surface loss is a separate rendering concern.

The realtime callback never waits for egui, a project mutex, allocation, disk or
inference. UI commands cross a bounded engine queue; engine status/meter snapshots
cross back without requiring each audio event to repaint. Use frame rates around
60 Hz while editing/playing and event-driven repaint when idle. Aggregate meter
updates. Heavy decode, project import and model jobs run on workers with bounded
progress/results and cancellation. `App::logic` can run while hidden with stale
last-frame input; never replay musical key events there. UI stalls or minimized
windows cannot stop the audio scheduler.

## Accessibility and distribution gates

The [egui accessibility guide](https://github.com/emilk/egui/blob/0.37.0/docs/accessibility.md)
provides labels, `WidgetInfo`, custom roles and an AccessKit test path. Custom
painting alone creates no usable note editor. Each actionable note/handle needs
an accessible role, name, state, bounds and supported action; the note list and
numeric inspector provide equivalent keyboard operations. Avoid announcing
meters/playhead changes every frame. Test focus order, visible focus, scale,
contrast, reduced motion, modal escape and IME input.

Before production acceptance, run VoiceOver on Mac and Narrator/NVDA on Windows
through create/open/import, transport, track selection, note inspection/edit,
save/undo and candidate review. Test keyboard-only full-song navigation and
offscreen notes. A generated tree and synthetic action test do not close these
gates. External display scale, window resize, GPU surface recreation, sleep/wake
and native dialog/menu focus also require real native runs.

Package the Rust executable and resources as `.app`/DMG on Mac, then Windows
executable plus NSIS/MSI. [cargo-packager 0.11.8](https://github.com/crabnebula-dev/cargo-packager)
(MIT OR Apache-2.0) can package eframe executables without adopting a webview; it
does not build the app unless configured to do so. Initial local `.app` packaging
can use ordinary bundle tooling. Signing/notarization, microphone privacy text
and entitlements are separate release work. See [Apple microphone key](https://developer.apple.com/documentation/bundleresources/information-property-list/nsmicrophoneusagedescription)
and [notarization](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution).
No signing identity, account acceptance, publication or paid service was used here.
Build and exercise Windows on Windows; an installed cross target does not prove
linking, device access, accessibility or installer acceptance.

Preserve license notices for the complete resolved graph. In particular,
[bundled fonts 0.37.0](https://crates.io/crates/epaint_default_fonts/0.37.0) carry
OFL-1.1 and Ubuntu-font-1.0 terms in addition to the crate's MIT/Apache terms.
UI dependency licenses do not establish rights to any model weights or plugins.

## Bounded proof and evidence

`experiments/rust-ui/` is an independent Cargo workspace with its own lockfile;
it does not change the root production manifest. Before compilation: local
Rust/cargo 1.98.1, Xcode 27.1, macOS 27.0.1, 128 GiB RAM, 16 CPUs and 655 GiB
free were observed. Only ordinary Rust crate dependencies were downloaded; no
models or weights. Compilation used two jobs and the proof target occupied about
292 MiB before the final lint pass.

Commands run from the repository root:

```sh
cargo test --manifest-path experiments/rust-ui/Cargo.toml --locked -j 2
cargo run --manifest-path experiments/rust-ui/Cargo.toml --locked -j 2
cargo clippy --manifest-path experiments/rust-ui/Cargo.toml --locked -j 2 -- -D warnings
```

Seven tests pass: accessible transport/pad/custom-note/waveform labels and
tessellated meshes; pointer selection preserving exact human note values;
AccessKit click selecting the same note; culling 100,000 offscreen notes from
both rendering and the accessibility tree; repeat suppression and release after
text gains focus; idempotent computer-key release on focus loss; and physical key
mapping with command-shortcut protection. The executable reports **one visible
note, 31 accessibility nodes and five painted primitives**.

This proof is headless and uses a synthetic waveform. It establishes current API
compatibility and narrow interaction behavior. It does not decode audio, drive a
GPU/window, benchmark frame time, edit a full song accessibly, monitor a device,
record, play audio or validate musical quality. Those remain explicit native
application and hardware acceptance gates.

### Native startup qualification correction

The first real Mac launch exposed a pre-window startup stall: process sampling
showed `main -> StudioApp::open -> NativeStore::open -> mkdir` on the main
thread, while the coordinator observed a pending removable-volume access
decision for the native application's own identity. A live process alone did
not establish a visible application.

The native entry point now returns a `LoadingApp` from the eframe creator with
no storage access. Its first UI pass draws the loading screen, then starts one
initialization worker. The UI polls a channel without waiting and requests
repaint every 100 ms while initialization remains live. It presents the selected
paths, explains that macOS may ask for external-drive access, and provides Close.
Success transfers `StudioApp` to the UI thread; errors and worker disconnection
produce a visible failure state. No automatic retries, permission changes or
thread joins occur on the UI thread. The synchronous `StudioApp::open` remains
available for isolated persistence tests. Actual native launch and the macOS
permission outcome must be rechecked by the coordinator after rebuilding.

## NSH evaluation

October 22, 2026. Requested candidate: [longbridge/nsh](https://github.com/longbridge/nsh).
Inspected exact upstream commit **`dc458b4ffb0916546b3ee7dd391d466a1a6e9815`**.
The workspace identifies itself as 0.2.0. The latest release observed was
[v0.2.0](https://github.com/longbridge/nsh/releases/tag/v0.2.0), published October 22
UTC; its peeled commit is `669b62b11449b2ab787b7c82649a392cbff25b8f`. The inspected
main snapshot is seven commits later, with no platform-backend or Cargo-manifest
changes in that comparison. Findings belong to the pinned snapshot.

**Recommendation: shortlist NSH for an isolated all-Rust native prototype.**
Its platform rendering and reactive components are relevant to a polished studio,
and the Rust-only path fits the production language requirement. The current
source has material keyboard, accessibility and close-lifecycle gaps for this
workstation. Retain the existing shell until a comparison proves those behaviors.
No NSH code was built, executed or installed during this investigation. No UI,
audio, runtime performance or Windows acceptance is asserted. The
[research receipt](../verification/rust-nsh-ui-research.json) records the snapshot
and evidence boundary. Two additional agents reviewed desktop integration and
readiness independently of the coordinator's canvas/architecture inspection.

### What fits

| Requirement | Verified source or documented support | OmniuSong consequence |
| --- | --- | --- |
| Rust-native application | The engine and Mac/Windows platform crates make JavaScript optional; Rust gallery components, handlers and canvas drawings are Rust. | Use platform dependencies with **`default-features = false`** and verify the actual app graph. Defaults otherwise enable QuickJS. A Rust-only app needs no browser or JavaScript VM. |
| Native Mac and Windows rendering | Mac uses AppKit/Core Animation/CoreText; Windows uses Win32/Direct2D/DirectWrite. Documented floors are macOS 14 and Windows 10 1809. | Relevant to Mac-first/Windows-second delivery. Windows controls are largely custom drawn; native rendering does not mean every widget is a stock system widget. Actual supported-OS checks remain necessary. |
| Custom music surfaces | Public Rust `canvas`/`Painter` record drawing operations; the Rust gallery includes canvas drawing and responder dragging. | Waveforms, notes, velocity/expression lanes and pads are feasible custom components. Dense music-editor performance remains unmeasured. |
| Reactive UI and native components | Components bind signals; `AppView` embeds in an owned native window. Both platforms expose custom native component hooks. | Reuse core/store/session/engine boundaries, with a new presentation layer and an app-owned lifecycle host. Keep audio work outside the UI runtime. |
| Extensible screens | Optional TSX modules can be delivered as signed packages. | This is a separate JavaScript-runtime choice, outside the current all-Rust design. UI packages do not supply VST3/CLAP hosting or MIDI 2/MPE transport. |

Primary sources: [Rust application guide](https://nsh.dev/docs/rust/),
[feature gates](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-macos/Cargo.toml#L14-L20),
[public Rust canvas constructor](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-core/src/ui/host.rs#L324-L335),
[Rust canvas example](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/examples/gallery-rs/native/src/demos/canvas.rs#L1156-L1179),
[native canvas implementation notes](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/docs/dev/canvas.md),
[Windows backend](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/docs/dev/windows.md),
[macOS backend](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/docs/dev/macos.md).

### Concrete gaps to prove or fix

1. **Held computer-piano notes need a native input bridge.** The macOS shortcut
   monitor listens only for KeyDown; Windows' module input path explicitly drops
   key ups. Ordinary press/release handling for native controls exists, but the
   public shortcut API is not a complete physical/logical musical key stream.
   No public surface-wide focus-loss event was found in this inspection. An
   adapter must preserve repeats, overlapping keys, releases after text takes
   focus, IME/shortcut protection and source-local cleanup on deactivation.
   [Mac shortcut source](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-macos/src/keymap.rs#L75-L120),
   [Windows key-up refusal](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-windows/src/input.rs#L922-L950).
2. **Accessibility descriptions are not sufficient action support.** The Mac
   backend explicitly states that custom `accessibilityActions` are not exposed.
   No press/increment/decrement forwarding overrides were found there. Windows
   has actual UI Automation action dispatch, but its range `SetValue` currently
   performs one increment/decrement toward the requested target. Exact editable
   values and actual VoiceOver/Narrator operation require qualification or fixes.
   [Mac accessibility source](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-macos/src/access.rs#L1-L27),
   [Windows range action](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-windows/src/access/uia.rs#L864-L898).
3. **Use an owned host for guarded Close/Quit.** No general application close-veto
   hook was found in the standalone runner. Mac close hides or stops the window;
   Quit finishes the app. Preserve existing Save/Discard/Cancel, uncertain-save
   disclosure, recording Finish/Stop and worker teardown in an owned host around
   `AppView`, or add a separately reviewed lifecycle API.
   [Mac delegate](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-macos/src/app.rs#L384-L445),
   [Windows window lifecycle](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/crates/nsh-platform-windows/src/window.rs#L769-L777).
4. **Plugin-window hooks are integration foundations.** NSView/HWND hooks exist,
   but cross-process plugin embedding, focus, resizing and crash isolation are not
   qualified. Native child HWNDs cover NSH-drawn overlays and do not appear in its
   offscreen drawings. Plan explicit window ownership and docking tests.
   [Windows layering limit](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/docs/dev/windows.md#L207-L209).

### Readiness and comparison with the current shell

The inspected [Rust-app CI definition](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/.github/workflows/rust-app.yml#L74-L115)
checks the gallery on Mac, Windows and Linux and rejects JavaScript engines in its
native dependency graph. The [Windows workflow](https://github.com/longbridge/nsh/blob/dc458b4ffb0916546b3ee7dd391d466a1a6e9815/.github/workflows/windows.yml#L97-L124)
also declares real-window input tests. These are inspected workflow definitions,
not fresh successful Actions results. Gallery assertions and role-mapping unit
tests do not establish musical drag behavior, screen-reader editing or dense
timeline rendering. The compatibility policy is primarily about shipped module,
prop and package surfaces; it is not proof that every Rust API is frozen. Pin a
revision and audit upgrades. The root license is Apache-2.0; a resolved
dependency/asset notice audit remains required. The moving stable toolchain and
absence of an explicit `rust-version` do not establish an exact minimum compiler.

| Choice | What it offers here | Remaining decision evidence |
| --- | --- | --- |
| Current egui/eframe | Existing native controls, custom drawing, drafts, service integration and bounded source/UI receipts. | Complete the current recording verification and visual/accessibility/performance gates. Its presence alone does not satisfy the UI quality goal. |
| NSH AppKit + Win32 | Rust reactive composition, native platform text/rendering, custom canvas and native-host embedding. | Key release/focus bridge, AX action forwarding, guarded lifecycle, actual dense-editor comparison and Windows parity. |
| NSH GPUI backend | Another documented host/rendering path, in a separate upstream workspace. | Separate dependencies, API and platform qualification. Its canvas is rasterized through tiny-skia; do not assume direct GPUI mesh performance or automatic resolution of NSH event/action gaps. |

OmniuSong's reusable backends make migration possible, but the application views,
draft orchestration, focus handling and egui-specific tests still require real
work. This is not a dependency-only replacement. Keep native audio scheduling,
capture evidence, project publication and high-resolution MIDI semantics out of
NSH components. Never rename a UI animation clock into an audio clock.

### Bounded prototype and decision gates

1. Create a separate `experiments/nsh-ui/` workspace with its own lockfile and
   target, pinned source and explicit no-JS features. Start with synthetic data
   and a fake session/engine service. Do not replace the current bundle or open
   live projects. Verify the resolved application graph separately from the
   heavier development CLI graph.
2. Use an app-owned Rust native host. Implement raw key down/up, repeat and focus
   notifications, a guarded Close/Quit flow and operable accessibility actions
   before connecting performance controls. Verify on Mac first; source or cross
   compilation cannot pass the Windows checks.
3. Build one designed studio screen: transport, sixteen pads, piano, editable
   numeric inspector, waveform, note/velocity/expression lanes and candidate
   panel. Reuse exact integer musical values and one shared viewport/hit-test
   transform. Render only visible notes and cached waveform summaries. Compare
   the same synthetic 100,000-note/offscreen fixture and long-source summaries
   with the existing shell at normal and high-density display scales.
4. Measure release builds: interaction/frame p50/p95/max, pan/zoom/drag, meters,
   memory and idle redraw/CPU behavior. A proposed 60-Hz active-view budget is
   16.7 ms p95 per frame on the declared machine/viewport; this is a prototype
   target, not an observed NSH result. UI worker delays must not block Stop or
   alter engine/capture semantics.
5. Exercise actual held-key overlaps/releases, pointer-capture cancellation,
   text/IME focus, focus loss, exact accessible edits, modal/close cancellation,
   resize/DPI and restart. Then connect a separately qualified software-output
   engine flow and verify drafts/history/archive behavior with synthetic copies.
   No native input stream is authorized by this experiment.
6. Choose migration only after the Mac proof and a real Windows run resolve the
   material gaps. If selected, extract toolkit-independent presentation state,
   add a parallel shell, migrate one workflow at a time and rerun each native
   acceptance packet. Preserve the existing backend and original data. Native
   plugin hosting and MIDI 2/MPE remain independent work.

The prototype is proposed, not implemented. This investigation changes no
production dependency or framework decision. The current optimized recording
walkthrough remains the next native acceptance task.
