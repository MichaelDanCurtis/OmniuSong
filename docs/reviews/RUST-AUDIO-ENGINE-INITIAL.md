# Initial independent native audio engine review

October 20, 2026. Independent architecture/realtime lane; implementation source, tests and device evidence will be recorded as they become available. This lane owns no engine production source, dependencies, app/device permission actions or Cargo execution.

## Initial disposition

The proposed isolated CPAL/CoreAudio worker with bounded prepared playback and a separate streaming journal writer is suitable for a first native slice, subject to the explicit constraints in [NATIVE-AUDIO-ENGINE.md](../contracts/NATIVE-AUDIO-ENGINE.md). It does not implement the full workstation, low-latency input response, calibrated duplex capture or thirty-minute qualification.

Initial findings sent to the coordinator and engine owner:

1. **Callback quiescence must precede final drain/seal.** Pinned CPAL 0.18.2 macOS stream-handle destruction is not an exposed join barrier: it signals monitor readiness while stream state is shared through an `Arc`. Require callback-producer destruction acknowledgment and bounded failure handling. Do not equate pause success, queue emptiness or a sampled active count with complete capture finalization. [Pinned stream source](https://raw.githubusercontent.com/RustAudio/cpal/v0.18.2/src/host/coreaudio/macos/mod.rs)
2. **Overflow evidence must survive a full queue.** Maintain source positions and terminal loss counters outside the ring; advance for rejected input. Final known trailing loss is journaled only after producer quiescence. Unknown hardware loss must remain unknown, not become contiguous successful PCM.
3. **Parent loss cannot imply a finished take.** The coordinator adopted an explicit amendment: only an explicit Finish may seal normally; parent/IPC loss stops input and retains unsealed original evidence. The shared protocol paragraph is amended accordingly.
4. **Preparation bounds are not recording limits.** 120 seconds/64 MiB applies only to playback preparation. Streaming capture must remain independent of those whole-buffer caps, and concurrent intermediate decode buffers must be included in preparation budgeting.
5. **Unsupported audible content must fail visibly.** Unknown instrument/codec, unimplemented expression, plugin or raw-event playback may not be silently omitted. Separate CPAL input/output streams do not establish musical alignment or authorize overdub claims.

## Review scope

Public `realtime`, `capture` and `render` primitives support the independent device-free suite described below. Source and regression findings are recorded here; central test results must be tied to the current source before claiming this slice passed. Source inspection does not establish universal callback safety or physical-device acceptance.

No actual device enumeration, capture, OS permission prompt or hardware test was performed by this lane. Existing recording/IPC/storage qualification remains evidence for those boundaries only.

## Pinned native dependency findings

The coordinator's resolved CPAL 0.18.2 uses coreaudio-rs 0.14.2. Reading the downloaded, unmodified registry source found two capture-path blockers that an application-only callback test cannot cover:

- **P1: native input resize allocates on the callback.** In `coreaudio-rs/src/audio_unit/render_callback.rs:622–657`, a changed `in_number_frames` triggers a stream-format property query and `Vec::resize` inside the native input closure, before the application's CPAL data callback. Growing beyond capacity allocates. A requested fixed hardware buffer does not prove that every native callback retains that length.
- **P1: input cleanup loses the allocation capacity after a resize.** The same closure tracks `buffer_capacity = vec.capacity()`, but `free_input_callback` at lines 750–754 reconstructs the allocation with `cap = mDataByteSize`. After a shrink, length is smaller than retained capacity; growth can also reserve excess capacity. Reconstructing a `Vec` for deallocation with that different allocation layout violates its safety precondition. This is a source-level unsafe-layout finding; no device failure or exploit was induced. [Pinned upstream source](https://raw.githubusercontent.com/RustAudio/coreaudio-rs/f07c02c7419328650112d4b276cffd785b0be4ee/src/audio_unit/render_callback.rs), [Rust raw-parts requirements](https://doc.rust-lang.org/std/vec/struct.Vec.html#method.from_raw_parts)

The file inspected has SHA-256 `3d1e78b8b6aee03e047d6d32feadc04fd94a2667c954ae994e4a83e99135028a`; its registry `.cargo_vcs_info.json` names upstream commit `f07c02c7419328650112d4b276cffd785b0be4ee`. The associated `audio_unit/mod.rs` hash is `11548aaefaf3e5a3ddf80dacd9e3f9d1167ddce1dc8dab6ae32849c57a81816f`; its destructor calls stop/uninitialize, ignores their errors, then frees callback resources. The CPAL Mac stream module hash is `8ac0856b57aa6bf54695b0048de3aef38e512fea1783c0dc61a0dff78d71e8b0`.

Before accepting live capture, use an audited narrow dependency repair or a native input adapter with preallocated maximum-sized storage, checked oversized-callback rejection, preserved allocation ownership/capacity and explicit callback lifetime acknowledgment. Keep dependency provenance and regression evidence. Application packet chunking alone cannot fix the upstream path, and successful constant-size synthetic callbacks cannot dismiss this finding. The coordinator owns the remediation and build decision; this reviewer made no dependency edits.

## Source review of the implementation

The initial source is now present in `crates/omniusong-engine`. This review covers application `realtime`, `capture`, `render`, `transport` and `worker` paths, plus the coordinator's narrow vendor repair and capture publication. Source/test coverage below is not a claim that the central test run or device qualification has passed.

The reviewed boundary uses a private self-executable worker, capacity-one client admission, 64-KiB engine envelopes, a 15-second total exchange budget and a 20-second child parent-contact watchdog. Initial inventory is bounded to 64 endpoints per direction and a 48-KiB serialized result. The worker requires an existing canonical native store and selects a genuine input endpoint before constructing an input stream. It rejects simultaneous playback/capture. Startup, inventory and Status do not start a microphone stream.

The application capture callback owns a fixed producer, copies at most 16,384 incoming frames into 256-frame × 8-channel `Copy` packets, and records source extent/loss through atomics. The 256-slot ring never overwrites. Once overflow is latched, subsequent delivered frames remain counted as rejected even if slots become free; control stops input on its next status/command cycle. Original finite float samples are retained without output limiting; i16/u16 conversion is exact into float32. Nonfinite samples, empty native-failure sentinel and unsupported extents latch a fault rather than produce accepted audio.

Normal finish requires a successful pause/drop followed by producer and error-handler closure destruction acknowledgments. Producer Drop explicitly destroys its SPSC endpoint before publishing the release flag. The final meter snapshot acquires this flag before final counters. The writer aggregates roughly 250 ms of packets with a 1-MiB ceiling, drains after the control transition, and compares predicted total gap units against ingress dropped frames before appending a final gap. Unknown device/input loss remains unsealed. The writer join is off UI; a stalled filesystem still relies on supervisor/child process retirement and can lose an uncommitted tail.

The prepared renderer validates WAV PCM16/24/32 or float32 and sealed PCM outside the callback. It rejects active unsupported sampler/voice/expression/raw-event material, missing originals, incompatible rates/channel conversion, declared source gaps, and arrangements beyond 120 seconds or the 64-MiB concurrent source/output budget. Built-in notes are a procedural audition tone with a drum envelope, not faithful hosted instruments or AI inference. Mono expansion is supported; resampling is pending. Timed output records the predicted final presentation endpoint and waits for a later callback clock to reach it before automatic stop. This is a source timing estimate, not measured audible/loopback completion.

Findings corrected during source review:

- Final capture counters were originally read before acquiring producer destruction. They now follow that acquire, avoiding a true acknowledgment paired with stale totals.
- A malformed output callback length originally risked shifting interleaved channel position. It now fills silence, reports a fault, and leaves the cursor unchanged.
- Public forgeable WAV metadata could overflow decoder offset arithmetic. Extent calculations are now checked, including payload range consistency.
- An accepted but undrained packet could have been mistaken for a trailing transfer gap. The reducer now validates predicted gaps before any final gap mutation.
- Per-packet journal commits did not implement the adopted aggregation target. The writer now batches outside the callback.
- Packet finite/range validation originally followed partial pending-buffer mutation. Complete validation now precedes gap/flush/append changes.
- Note-track asset references could have been interpreted as both synthesized notes and a placed audio source. Active note assets now reject as an unimplemented sampler.
- Ending output immediately when PCM was copied could truncate the scheduled device tail. The worker now waits on the timed callback presentation endpoint.
- A backwards observed capture-start clock could be hidden by aggregate timestamp clamping. Ingress now latches a clock fault while counting delivered/rejected frames; the reducer rejects backwards packet starts and gap observations before mutation. Inferred buffer-end overlap alone is not used as a strict clock-regression test.
- A failed pause or destruction timeout left a worker capable of admitting another stream despite an unproven old lifetime. The generation now retires and exits without normal sealing or another device admission. Failed input/output pause deliberately preserves stream ownership until that bounded exit; output start failure also retires instead of invoking uncertain teardown.
- A terminal heartbeat failure could be discarded behind an occupied one-slot completion queue. The supervisor now retires the child, marks admission dead and retains the terminal event behind the normal completion; UI receiver closure unblocks delivery.

## Vendored remediation and independent fixtures

The coordinator vendored the exact upstream package with original licenses and `UPSTREAM.json` file/checksum provenance. The patch removes property-query/resize/raw-parts reconstruction from the input callback, allocates fixed capacity before activation, and retains a unique original allocation owner. A follow-up alignment review found that `Vec<u8>` did not itself guarantee typed f32/f64 sample-view alignment under every valid global allocator. The final selected backing uses `Vec<u64>` through checked `input_storage`; its original word allocation is destroyed intact. Native render failure, zero/oversized frame count produces both zero bytes and zero frames for the application unknown-loss sentinel.

The independent vendor file `vendor/coreaudio-rs/src/omniusong_review_tests.rs` contains three pure regressions: checked byte bounds; actual patched storage construction with f32/f64 alignment, stable pointer/length/capacity through variable/zero views and zero scoped hot-path allocation/deallocation; and typed/raw zero-frame sentinel consistency. It does not invoke AudioUnitRender or any device. Native AudioUnit stop/uninitialize error handling and CPAL's exceptional allocation/locking paths remain outside these pure tests and are not declared universally realtime-safe.

`crates/omniusong-engine/tests/engine_review.rs` contains 18 independent cases. They cover queue configuration, scoped application allocations/deallocations, variable callbacks/channel order, exact overflow accounting, producer lifetime, invalid/native-failure input, output tail/silence, exact journal gaps and retained samples, incomplete-drain rejection, abort/unknown fault/missing acknowledgment, pending-data nonmutation and aggregation, WAV numeric/structural bounds and forged metadata, planar interpretation, malformed output shape, saved note timing/mute/unsupported/preparation limits, sealed clip trim/signed placement/fade/source-file preservation, and timed presentation endpoint behavior.

`crates/omniusong-engine/tests/engine_process_review.rs` adds three independent actual subprocess cases through the inherited private launcher. A real `run_worker` handles Initialize/Status/Stop without devices and retires after client drop. A test peer authenticates through the same launch protocol, then injects EOF, wrong response identity or wrong result kind; the client must report the pending identity once, refuse new admission and retire the child. A third case leaves a normal completion queued while the child exits, then requires the completion followed by the retained terminal error; the one-slot reply queue must not discard it. Its one ignored test is only the subprocess entrypoint. No fixture enumerates audio endpoints or opens input/output streams.

## Capture attachment review

The read-only review of `NativeStore::attach_capture` and session `Command::AttachCapture` found no additional blocking defect. The store validates canonical publication intent and authenticated operation replay before touching the stage. Exact registered originals are physically verified and reused when the stage is absent; otherwise sealed stage identity is verified before durable ingestion. The later publication transaction checks expected revision. A conflict preserves the newly registered original for explicit retry. Session replay loads the latest saved project rather than replacing later edits with the original receipt document. The coordinator owns three store attachment cases and one real storage-worker case; their central outcomes must be recorded separately.

## Limits and acceptance status

The final central software gate passed as recorded below; this reviewer ran no Cargo command and no device action. Device names/IDs and rate/channels are visible, but actual hardware buffer frames/latency are not yet in the status API. The native dependency repair passed its five pure central regressions; explicit input-device stop/error qualification remains pending. Pure allocation fixtures cover only executed application/view paths, not all CPAL, CoreAudio or driver behavior. The current first backend is Mac-only. Stalled OS calls, external-drive removal, forced process kill and power loss can leave an unknown uncommitted tail; the existing journal recovery distinction must remain visible.

Under-10-ms software response, 30-minute dropout-free capture, playback/input duplex alignment, sample-rate conversion, long-arrangement streaming playback, arbitrary codec/sampler/plugin rendering, CoreMIDI/MIDI2/MPE, AI inference under audio load and Windows remain separate required work. These are initial-slice omissions; none is waived by this review.

## Reviewed source fingerprints

SHA-256 of the reviewed final clock/retirement source used by the coordinator's integrated gate. A later source change requires its own review/gate; these are source identities, not executable or hardware qualification.

| File | SHA-256 |
|---|---|
| `crates/omniusong-engine/src/lib.rs` | `8f198507e7c1d0837f90835721f60aced608b180188c75a24b2935ffe30ea444` |
| `crates/omniusong-engine/src/realtime.rs` | `bc9c8b36f1242e226810569e4b896aea92f4a117efa9dff08bdfed43325bf8fe` |
| `crates/omniusong-engine/src/capture.rs` | `f7f3fa854a10adf336ef399f2088637b33746da4aba1ff89b0a7983e1b059004` |
| `crates/omniusong-engine/src/render.rs` | `7ff04b35aca350f08e79a4f7ed122e4d2e38ac74bfc96b1c75f70183f65e57aa` |
| `crates/omniusong-engine/src/transport.rs` | `69d8311ecc5f6d370ad3270fc5ffd13b5fe43bd0ae18767d5d0b178c84599c16` |
| `crates/omniusong-engine/src/worker.rs` | `84ee27d9c6e1b10e9d06e8e55f7548c466e1e365795d76a7906a5c79c67ae462` |
| `vendor/coreaudio-rs/src/audio_unit/mod.rs` | `26f90e138248daed4815f9d70b4e70b9b1d5801fb924e1feb0c17a7ccee09ef9` |
| `vendor/coreaudio-rs/src/audio_unit/render_callback.rs` | `80802960e917fa3ad9cdd22453ac8f0a0b7e0e8cc563516b9ab137b9bbb18469` |

## Final central software gate

The coordinator ran the build/test commands; this lane independently read the completed logs after the final source freeze. `cargo test --workspace --all-features --locked` passed **246 tests, zero failures, one intentionally ignored subprocess helper**, across 30 test-result groups. The engine portion includes 17 owner tests, 18 independent callback/capture/render cases and three independent actual private-process cases. The vendor filtered `omniusong_` suite passed five tests, including all three independent aligned-storage/view cases. All-target/all-feature Clippy with warnings denied completed cleanly; the coordinator also reports the final formatting check clean.

Evidence is retained in `.runtime/rust-audio/workspace-tests-final.log` (SHA-256 `53feec500fb0448b061c3fe7ea393bef84ae9bd7e991f4a34cae59993b89c13a`), `.runtime/rust-audio/vendor-input-tests.log` (`13c33a46d97e30cc462ca6e25e6117f5ec5127701498e8d5e8e05806389f058a`) and `.runtime/rust-audio/clippy-final.log` (`1db534991bfdb7149ff048ef89813bc1fe2c49d00dcb7009341f75bb7719be3b`). The earlier invalid asset-ID test fixture was corrected to reach its intended sampler rejection. An earlier parallel recording test encountered a transient `Owned` reopen failure while helper processes spawned; its owner serialized those independent fixtures. Inherited-lock scheduling is an explanation inferred from that test context, not a qualified production concurrency/power-loss result.

No concrete source blocker remains in this reviewed first slice. This is acceptance of the bounded software implementation and regression evidence. Actual audible output, microphone authorization/delivery, native stop/error behavior, latency, 30-minute recording and other full-goal gates remain separate. The coordinator reports a signed Mac bundle hash `81b00cb49c0abae70ee5b8b6df9654dcc6f1fdea11c247ba884c61c829b1f0b5`; at this review checkpoint its session worker was waiting in an OS external-drive open. This lane did not operate the app or qualify hardware. Later UI/device evidence must be recorded independently rather than inferred from the signature or software tests.
