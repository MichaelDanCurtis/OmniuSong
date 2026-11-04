# Native Rust model adapter research

Checked October 20, 2026. This document evaluates real inference for the native migration. It is a feasibility and qualification plan, not an inference receipt. No new weights were downloaded, checkpoints loaded, model gates accepted, paid services called, or recordings uploaded during this investigation.

Production job orchestration, input projection, validation, provenance, candidate decisions, and storage should be Rust. Native inference libraries may sit behind narrow Rust-owned boundaries. Python and Node workers in the current preview are comparison references; wrapping their processes does not meet the native migration requirement.

## Recommended next adapter

Implement **Basic Pitch ONNX on CPU in a Rust worker first**. Its existing model is 230,444 bytes, has a known checksum, and already has a synthetic fixture and a useful reference output. This closes a real musical role without a new checkpoint download or a model architecture port. The work includes preprocessing and note decoding, not just running the graph. Keep the first limit at the existing 120 seconds and four inference threads; measure before increasing either.

Next, port the existing short Magenta Small candidate path through the **official native `magentart::core` library**. Start with an offline text/note-guided render. Its working native streaming interface makes it the strongest accompaniment candidate, but sustained live accompaniment requires a separate device qualification. A native shell, a runtime that loads, and a model that completes one render are three different results.

## Local inventory and evidence

Read-only inspection found macOS 27.0.1 arm64, 128 GiB physical memory, 16 logical CPUs, and about 655 GiB available on the project volume. `rustc 1.98.1`, `cargo 1.98.1`, Xcode clang, CMake 4.3.0, and FFmpeg are present. These are resource and toolchain observations, not guarantees about another process's available memory or model performance. No native model build was attempted.

| Existing asset | Observed size | Identity and evidence | Native migration implication |
| --- | ---: | --- | --- |
| Basic Pitch `nmp.onnx` | 230,444 bytes | SHA256 recomputed: `2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec`; [`basic-pitch.json`](../verification/basic-pitch.json) | Retain graph; replace Python decoding and Node process wrapper |
| Aria `model-gen.safetensors` | 2,634,170,640 bytes | Recorded SHA256 `5e6b07ee2680f0044deea30da613d89b12e3e30d012d1812b388d6b8d8306c90`; model revision `5a1ef36c5007ee10fbe57d61d96725a7244bd2b2`; [`ARIA-QUALIFICATION.md`](../ARIA-QUALIFICATION.md) | Retain checkpoint as port target; no native architecture implementation qualified |
| Magenta Small graph/state and four style resources | 1,042,112,092 bytes total | Current file sizes match [`magenta-assets.json`](../../workers/magenta-assets.json); source `694a545e4ba0b88bf1150137b129582166d3e07f`; model `010aa0dcb0dfd27b24f0ad07b4dad63e8f9521cc` | Retain assets; replace Python with native library; audio-style/prefill assets absent |

Large checkpoints were not rehashed here. Their hashes above come from existing qualification/manifests; file size is weaker evidence than a fresh hash. The Basic Pitch checksum was verified directly because its file is small. Environment sizes were approximately 495 MiB Basic Pitch, 531 MiB Aria, and 909 MiB Magenta, separate from model storage. Do not bundle those Python environments into the native application.

The existing receipts establish synthetic Basic Pitch inference, Aria piano continuation, and short offline Magenta generation through their previous runtimes. They establish neither native Rust inference nor human performance quality. The optional Ollama lyric path was not qualified. The migration coordinator's fresh October 20 baseline reports 114 tests passing and two failing: real Aria and Magenta tests fail to import `aria` and `magenta_rt`; the real Basic Pitch ONNX test passes. Read-only inspection confirms Aria/AriaUtils editable import mappings and Magenta's `.pth` still point to the old `/Volumes/Main External/OmniuSong` location. These environments were not repaired here. Older qualification receipts must not be presented as current Aria/Magenta runtime readiness. References: [`README.md`](../../README.md), [`MODEL-RESEARCH.md`](../MODEL-RESEARCH.md), [`workers/`](../../workers/), [`ARIA-QUALIFICATION.md`](../ARIA-QUALIFICATION.md), and [`MAGENTA-QUALIFICATION.md`](../MAGENTA-QUALIFICATION.md).

## Role matrix

Every role remains **native-unqualified** until an OmniuSong receipt proves its complete path. “Retain” refers to the model or musical semantics, not to retaining its Python service.

| Required role | Concrete Rust/native route | Retain or replace | Exact integration/porting gap and qualification |
| --- | --- | --- | --- |
| Lyric revision | Rust Candle Qwen3 inference plus Rust `tokenizers`; CPU baseline, then Metal. A pinned `Qwen/Qwen3-0.6B` is a bounded smoke candidate; a larger explicitly selected checkpoint needs its own musical evaluation. | Retain explicit model selection and revision candidates; replace Ollama HTTP dependency for the default native path. | Implement the exact chat template, thinking-mode policy, stop tokens, sampling, bounded output and structured revision validation; prove locked lines and originals survive. No selected Qwen weights were found in this project's inventory. Small-model fluency is not lyric quality. Candle has a Qwen3 model and example, while Qwen's selected model card identifies Apache 2.0. [Candle implementation](https://github.com/huggingface/candle/blob/12bb5d1f6350f4ea9827d9086f213feec1f0af6a/candle-transformers/src/models/qwen3.rs), [example](https://github.com/huggingface/candle/blob/12bb5d1f6350f4ea9827d9086f213feec1f0af6a/candle-examples/examples/qwen/main.rs), [model card](https://huggingface.co/Qwen/Qwen3-0.6B) |
| Audio to MIDI | Rust `ort` with a pinned native ONNX Runtime CPU library and existing Basic Pitch graph. | Retain graph, original audio, estimated curves, clip-position semantics; replace `basic_pitch_worker.py` and Node wrapper. | Port resampling, overlap/window logic, activation decoding, contour-to-bend estimation, time correction and output conversion. Compare raw activations and decoded notes against frozen reference fixtures, then qualify authorized human voice/instrument input. CoreML is optional after CPU parity; graph partitioning and fallback must be observable. [Basic Pitch](https://github.com/spotify/basic-pitch), [`ort`](https://github.com/pykeio/ort), [CoreML provider](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html) |
| Symbolic continuation | Rust implementation of Aria's transformer/tokenizer using Candle; alternatively an owned C ABI around native MLX operations. | Retain pinned Aria generation checkpoint and source-projection policy; replace Aria Python/MLX orchestration. | Safetensors loading alone does not implement Aria. Port parameter mapping, LayerNorm, QKV attention, RoPE, gated feed-forward layers, causal masking, KV-cache boundaries, tokenizer/vocabulary, sampling and detokenization. Preserve the current explicit attention-cache/scalar-offset fix. Prove logits/tokenizer parity before seeded musical continuation and source isolation. No ready Aria Rust backend was established. [Official model/runtime](https://github.com/EleutherAI/aria), [checkpoint](https://huggingface.co/loubb/aria-medium-base), [native MLX C bridge](https://github.com/ml-explore/mlx-c) |
| Full-song generation | First native comparison: Rust-owned adapter to `ServeurpersoCom/acestep.cpp` GGML library/CLI. Pure Rust comparison: `Marenz/ace-step-rs` with Candle, after repairing its build dependencies. | Retain ACE-Step as a model candidate; replace proposed Python orchestration. Keep MiniMax as an independent alternative, with its own terms and evidence. | Pin LM/text encoder/DiT/VAE and tokenizers, audit conversion provenance, prevent automatic downloads, enforce short initial budgets, cancellation and WAV validation. `acestep.cpp` documents Metal; this Mac has not run it. `ace-step-rs` explicitly says Metal is untested; its checked manifest has absolute developer-local Candle paths and default CUDA, so it is not a drop-in Mac dependency. Qualify a short lyric-conditioned render before full songs, then structure/lyric/control quality. [Native C++ project](https://github.com/ServeurpersoCom/acestep.cpp), [Rust project](https://github.com/Marenz/ace-step-rs), [manifest](https://github.com/Marenz/ace-step-rs/blob/f829d09d38a09ae0870aefb094d12582ce7e9dab/Cargo.toml) |
| Separation | Rust-owned C ABI or isolated native executable around `sevagh/demucs.cpp` HTDemucs inference. CPU is the initial portable baseline. | Retain Demucs four-stem role; replace Python inference rather than the original recording. | Select and pin converted model with traceable source checksum, normalization/STFT/split-overlap policy and stem ordering. Verify length, channel alignment, mixture reconstruction error and listening; stems need independent editable tracks and original-mix provenance. Native project trades speed for lower memory and is not a measured real-time separator. Original Meta repository is archived. [Native implementation](https://github.com/sevagh/demucs.cpp), [upstream model and license](https://github.com/facebookresearch/demucs) |
| Sample/instrument creation | Near-term real model samples: native Magenta Small clip generation followed by Rust trimming, root-note/zone/envelope/loop mapping. Text-to-SFX comparison: `audio.cpp` Stable Audio 3 Small SFX behind its C API. Learned performance-controlled timbre: RAVE TorchScript through native libtorch/`tch`, separately licensed. | Preserve procedural instrument tools with their procedural label. Add true model-produced samples; retain RAVE/DDSP as separate neural-instrument research. | Generated PCM is not a playable instrument. Qualify transients, pitch, looping, velocity zones, resampling and note-off, then save the sample map reversibly. Stable Audio access is gated and unapproved; RAVE upstream's noncommercial terms prevent assuming a default commercial bundle. No chosen RAVE checkpoint is installed or licensed here. DDSP's reference TensorFlow pipeline needs a graph export or native architecture/DSP port. [Native SFX path](https://github.com/0xShug0/audio.cpp/blob/main/docs/models/stable_audio.md), [SFX gate/terms](https://huggingface.co/stabilityai/stable-audio-3-small-sfx), [RAVE](https://github.com/acids-ircam/RAVE), [DDSP](https://github.com/magenta/ddsp) |
| Singing synthesis | Rust `ort` executing pre-exported DiffSinger variance/acoustic graphs and a compatible ONNX vocoder. | Retain score, phoneme, pitch and expression controls; implement a native inference adapter rather than embedding OpenUtau or a Python server. | Select permitted voicebank/vocoder first. Implement language phonemization/dictionaries, note-to-phoneme alignment, duration/pitch/variance controls, diffusion/flow sampling, acoustic/vocoder shape conventions and resampling. Pin every graph/config. Prove one phrase, then three independently editable harmony parts; choir arrangement belongs to OmniuSong. Code licensing does not license voices. [Deployment format](https://github.com/openvpi/DiffSinger/blob/main/docs/GettingStarted.md), [OpenUtau integration reference](https://github.com/openutau/OpenUtau/wiki/DiffSinger-support), [vocoder terms](https://github.com/openvpi/vocoders/releases) |
| Interactive accompaniment | Rust-owned native bridge to official `magentart::core::RealtimeRunner` on Apple Silicon, with Rust session/control ownership and bounded audio transfer. | Retain Small model and note/style conditioning; replace `magenta_worker.py`. | First build and qualify offline text/note generation. Then prove stream state reset, prompt response, note handoff, buffer underruns, cancellation, device latency and simultaneous recording. Cached assets support the current text/note experiment; audio style encoding and audio prefill are additional capabilities with additional assets. Native upstream's hardware claims do not establish OmniuSong latency. Windows needs another qualified native backend or an explicitly visible unavailable capability. [Official native core](https://github.com/magenta/magenta-realtime/blob/694a545e4ba0b88bf1150137b129582166d3e07f/core/README.md), [runner](https://github.com/magenta/magenta-realtime/blob/694a545e4ba0b88bf1150137b129582166d3e07f/core/include/magentart/realtime_runner.h) |

## First adapter: Basic Pitch details

Use the existing graph without conversion and use CPU execution first. `ort` provides Rust access to native ONNX Runtime; pure Rust alternatives such as tract remain possible later, but their operator compatibility must be demonstrated with this exact graph. Do not interpret a runtime's general ONNX support as validation of a particular model.

The reference implementation contains several musically visible transformations that must be ported explicitly:

1. Decode bounded WAV, downmix and resample to 22,050 Hz mono; record the selected source window and its original rate/channel layout. Changing the resampler can move activations, so retain the preprocessed fixture as a parity boundary.
2. Use 43,844 input samples per window, 30 overlapping output frames, 7,680 overlapping input samples and a 36,164-sample hop. Include leading half-overlap padding, zero-padding of the last window, trimming 15 output frames at both ends, concatenation and original-length crop. Validate graph input/output names, types and shapes from metadata.
3. Decode onset, note and contour activations with the frozen 0.4.0 policy: onset threshold 0.5, frame threshold 0.3, minimum note length 127.70 ms, inferred onsets and the Melodia residual-energy pass. Preserve the upstream frame-time correction. Simply thresholding every active pitch gives different notes.
4. Estimate pitch curves from three contour bins per semitone. Preserve raw model estimates separately from the existing explicit two-semitone UMP projection. Activation amplitude is an estimated velocity, not measured human velocity; newly generated IDs may differ while numerical music values match.
5. Convert source-relative seconds to ticks only after decoding, then apply the clip's arrangement offset exactly once. Keep originals, trims, expression, locks and source hashes immutable. Return a candidate; accepting it is a separate project transaction.

The algorithmic references are upstream [`inference.py`](https://github.com/spotify/basic-pitch/blob/main/basic_pitch/inference.py) and [`note_creation.py`](https://github.com/spotify/basic-pitch/blob/main/basic_pitch/note_creation.py), plus the locally installed 0.4.0 source used by the existing worker. Pin the actual source version in the implementation receipt, rather than copying a moving `main` branch. Preserve its Apache notices in the port.

The first real local gate should transcribe the existing 3.2-second four-tone fixture and verify pitches 60/64/67/72 plus numerical onset/duration/contour tolerances declared before running. Add fixtures for silence, repeated same-pitch notes, stereo/resampling, notes crossing a model-window seam, and a nonzero clip offset. Compare activations independently of decoder output to distinguish graph/runtime drift from a decoder bug. Use exact hashes for artifacts, numerical tolerances for floating-point activations, and musical event comparisons that ignore UUIDs. An inference result with the right pitches still does not establish singing or dense-polyphony quality.

## Native Magenta integration constraints

The pinned source already contains `core/`; the current local checkout is sparse and includes only `docs` and `magenta_rt`. Its native headers/source were inspected with `git show`, without changing the checkout. Thus the missing working-tree directory is not evidence that a native implementation must be invented.

At that revision, `magentart::core` is a C++20 static library linked to MLX, TFLite and SentencePiece plus Metal/Accelerate/Foundation. The root CMake pins MLX v0.31.1, SentencePiece v0.2.0 and TensorFlow v2.21.0, explicitly disables MLX Python bindings, and targets macOS 14+. It also configures example UI targets with npm. Create a narrow core-only build definition with pinned dependencies; do not import example UI targets into the Rust product. CMake 4.3.0 is present locally, while upstream setup recommends `cmake<3.28`; verify compatibility in an isolated native build before changing any toolchain. [Pinned build definition](https://github.com/magenta/magenta-realtime/blob/694a545e4ba0b88bf1150137b129582166d3e07f/CMakeLists.txt)

The existing six assets contain the Small `.mlxfn`, its initial state, text encoder, optional mapper, quantizer and SentencePiece model. Native `init_assets` attempts optional audio-preprocessor/music-encoder loads; those two absent files must produce a truthful unavailable audio-conditioning capability. Audio prefill additionally needs SpectroStream/prefill graphs. Verify the exact native execution path accepts the selected six-asset set; do not silently fetch a broader model package. [Pinned engine source](https://github.com/magenta/magenta-realtime/blob/694a545e4ba0b88bf1150137b129582166d3e07f/core/src/mlx_engine.cpp)

Expose opaque handles and fixed-layout sample/note/control buffers through a small C ABI. Catch C++ exceptions before the ABI; free buffers on their owning side. Lifecycle calls and prompt encoding stay off the audio callback. Native generation produces 1,920 stereo samples at 48 kHz per 40 ms frame. Drain the Metal autorelease pool during long inference loops, and keep lifecycle/generation thread rules explicit. For live output, only the nonblocking ring-buffer read belongs in the audio callback. Its underruns must reach the receipt and UI.

The pinned `RealtimeRunner::set_audio_prompt(index, path)` is explicitly marked as producing a deterministic fake embedding for a nonempty path. **Do not expose this method as real audio conditioning.** The sample-based method requires genuine decoding/resampling and the missing audio models. Restrict the first bridge to verified text/note functionality. Native/Python RNG, warmup, prompt masking and conditioning defaults may differ, so byte-identical stochastic PCM is not a required parity assumption; document and test those differences before comparing outputs.

## Typed worker and provenance boundary

Use one Rust request/result protocol for all model roles. A worker owns inference resources, never the project store. Prefer isolated native worker processes for expensive or less mature adapters so cancellation and a native crash cannot destroy capture or saved sessions. An in-process adapter is reasonable only when its allocation, cancellation and failure behavior are understood.

```rust
// Conceptual interface; not a dependency or implemented API.
struct ModelRequest {
    protocol_version: u32,
    job_id: JobId,
    project_revision: u64,
    role: ModelRole,
    model: PinnedModelId,
    input: RoleInput,       // tagged lyrics, score, or immutable asset/window refs
    limits: InferenceLimits, // time, frames/tokens/notes, threads, memory budget
    seed: Option<u64>,
}
struct ModelResult {
    candidate: RoleCandidate, // typed text, notes, audio, stems, voice or sample map
    provenance: InferenceReceipt,
}
```

Include model/source/runtime/build identities, every asset's size/hash, actual execution provider/device, projection policy and lost fields, sampler/seed, input/source-scope hashes, source revision, wall/load/inference timing, peak-memory measurement definition, warnings and output hashes in `InferenceReceipt`. Keep install presence, asset validity, graph load, successful inference, musical qualification and live qualification as separate states. A checksum from the worker must be independently checked by its Rust host.

Requests contain asset IDs and bounded windows, not arbitrary paths or shell commands. Resolve inputs from a trusted asset store, supply private output directories and atomically publish only validated artifacts. Bound diagnostics and protocol frames; enforce cancellation, timeouts, queue/memory concurrency and restart interruption. Inference itself must not make network requests or silently choose/download a model. A download/install action is distinct and keeps the exact license/access decision visible.

Keep the established audition/accept/discard semantics, stale-project detection and idempotent acceptance. A successful worker cannot replace a locked source or approve its own output. For live accompaniment, separate the control protocol from preallocated audio buffers and timestamp both control input and generated frames; save the generated performance and control trace as new material.

## License and access decisions still open

These are concrete source observations for artifact selection, not a completed distribution review.

| Artifact family | Observed terms | Required selection decision |
| --- | --- | --- |
| Basic Pitch and Aria | Upstream identifies Apache 2.0 model/tooling licensing | Preserve notices and pin selected model/source; verify native runtime dependencies separately |
| Magenta RealTime 2 | Apache 2.0 code; CC BY 4.0 weights plus model-card usage terms | Carry attribution and exact model card; define model installation and distribution policy. Existing ungated download is not native qualification. [Pinned card](https://huggingface.co/google/magenta-realtime-2/blob/010aa0dcb0dfd27b24f0ad07b4dad63e8f9521cc/README.md) |
| ACE-Step 1.5 | Official model card identifies MIT; both shortlisted native projects identify MIT | Audit selected auxiliary Qwen assets, conversions and dependencies; no weights installed here. [Official card](https://huggingface.co/ACE-Step/Ace-Step1.5) |
| MiniMax Music 3 | Community License is now directly available, superseding the earlier “no license label captured” uncertainty | Terms include UI attribution, a USD 20M aggregate yearly revenue authorization threshold and safeguards/AUP obligations. Inspect the complete exact license before selecting/distributing. Official runtime documents two CUDA GPUs and non-streaming; native community ports are additional implementations to qualify. [License](https://huggingface.co/MiniMaxAI/MiniMax-Music3/blob/c2509fd6b60d1ae169cd1df27f78a53174ba17e8/LICENSE), [official runtime](https://github.com/MiniMax-AI/MiniMax-Music3) |
| Stable Audio 3 Small SFX | Gate asks to share Hugging Face information and accept Stable Audio Community and Gemma terms | No acceptance performed or inferred. Keep unavailable until the user authorizes the exact access terms and a permitted artifact is selected. [Model access page](https://huggingface.co/stabilityai/stable-audio-3-small-sfx) |
| RAVE | Upstream repository license is CC BY NC 4.0 | Research candidate; no automatic commercial product inclusion. Each checkpoint also needs terms and provenance. [License](https://github.com/acids-ircam/RAVE/blob/master/LICENSE) |
| DiffSinger | OpenVPI engine is Apache 2.0; voicebank and vocoder rights are separate | Select authorized voices and a compatible vocoder. The cited community NSF-HiFiGAN release has CC BY NC SA 4.0 pretrained weights, so it cannot be assumed to be the unrestricted default. [Engine](https://github.com/openvpi/DiffSinger), [vocoder release](https://github.com/openvpi/vocoders/releases) |
| Demucs | Original upstream identifies MIT for Demucs; native `demucs.cpp` is MIT | Verify the selected checkpoint's terms and trace every converted checkpoint to its upstream source; preserve both sets of notices. [Upstream](https://github.com/facebookresearch/demucs), [native license](https://github.com/sevagh/demucs.cpp/blob/main/LICENSE) |

Native `audio.cpp` has a C API and documents ACE-Step, HeartMuLa, MiniMax Music 3 and Stable Audio families. It is a useful later shared-runtime comparison, not proof that all those models work on this Mac or satisfy their individual licenses. Prefer the bounded Basic Pitch and official Magenta paths before adding a broad runtime dependency. [Project/runtime surface](https://github.com/0xShug0/audio.cpp), [music routes](https://github.com/0xShug0/audio.cpp/blob/main/docs/music_generation.md)

## Research pins and implementation order

The following remote HEADs were recorded with read-only `git ls-remote` on October 20, 2026. They are research snapshots, not approved dependency pins or build results.

| Source | Observed commit |
| --- | --- |
| `pykeio/ort` | `5c564c58507f35fa4adc387a79fce1016e73956b` |
| `huggingface/candle` | `12bb5d1f6350f4ea9827d9086f213feec1f0af6a` |
| `Marenz/ace-step-rs` | `f829d09d38a09ae0870aefb094d12582ce7e9dab` |
| `ServeurpersoCom/acestep.cpp` | `de858d8e1a94f0e1939cf9bdd8ff7d9631953077` |
| `0xShug0/audio.cpp` | `c677d5e67ced4bfd17e2e38c4833f7388cd90759` |
| `openvpi/DiffSinger` | `76b75dec8a5233afa47ef0e1377d25a080af3289` |
| `sevagh/demucs.cpp` | `f1206e9adeea103aef4a636b9e62297cf1f8e34e` |

1. Build Rust worker/provenance/candidate boundaries and native shell status reporting. Port Basic Pitch and prove the real local inference gate above.
2. Build only native Magenta core in an isolated dependency tree; qualify 2–4 second text/note output against the existing scope, then longer live device/recording tests. Model load and warmup never run on the audio callback.
3. Add lyric inference after explicit checkpoint selection; port Aria tokenizer/model while preserving the established source projection and candidate lifecycle.
4. Compare native ACE-Step implementations using a short original brief before any full-song claim. In parallel with implementation scheduling, resolve DiffSinger voice/vocoder and sample-model licensing; qualify separation, singing and playable model-generated instruments independently.
5. Requalify each shipped role on Windows with the same project/assets and measured output. Mac-native MLX success does not establish Windows support. Keep any missing role visible and incomplete until its own real inference, preservation and musical gates pass.

The full native migration remains incomplete while any required role relies on an unapproved Python/Node service, a procedural substitute, a disabled placeholder, or an unqualified model path.
