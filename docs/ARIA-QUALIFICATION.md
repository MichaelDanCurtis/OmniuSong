# Aria MLX qualification

October 15, 2026. This checkout now runs an actual local symbolic piano model alongside Basic Pitch transcription. Aria generation now runs through the bounded worker and the studio candidate queue, with source-bar selection, token budget, seed, audition, acceptance, and discard. The saved **Aria piano · local model preview** session lets you play and inspect this qualification result today.

## Exact model and runtime

The [official Aria repository](https://github.com/EleutherAI/aria) supplies its MLX implementation. The generation checkpoint is `model-gen.safetensors` in [loubb/aria-medium-base](https://huggingface.co/loubb/aria-medium-base), whose model card identifies Apache 2.0 licensing. The repository metadata reported it ungated. No account authentication, model-gate acceptance, training, or external inference was used.

| Item | Qualified identity |
| --- | --- |
| Model revision | `5a1ef36c5007ee10fbe57d61d96725a7244bd2b2` |
| Checkpoint bytes | 2,634,170,640 |
| Checkpoint SHA256 | `5e6b07ee2680f0044deea30da613d89b12e3e30d012d1812b388d6b8d8306c90` |
| Aria code | `c2f67bc90b49335543c0249fe00fb84a9337faf8` |
| Aria utilities | `7c815755c6889a784b6a1b3343e9e8daa3e7d128` |
| Python / MLX / Torch | 3.11.15 / 0.25.2 / 2.8.0 |
| Runtime / model locations | `.runtime/aria`, `.runtime/aria-model` |

The official README's standalone `aria-medium-gen` link returned an unavailable response. The official base model card points to the generation file used here. Only that checkpoint and its README were downloaded, about 2.63 GB; the isolated Python environment occupies approximately 532 MiB. No second checkpoint or training dataset was fetched. Available disk was approximately 745 GiB; system memory was 128 GiB with low pressure before the run.

## Actual inference result

The original input is a deliberately simple, procedural 12-note phrase stored in `verification/aria-prompt.json`. The model generated **21 additional complete notes from 64 new tokens**, after 38 prompt tokens. The receipt run took **2.500 seconds** including process setup, imports, checkpoint verification, and loading; its generation phase took **0.560 seconds**. An earlier successful run measured 2.760 seconds total and 1.134 seconds generation. These are warm-filesystem, single-fixture observations, not live accompaniment or general performance benchmarks.

MLX reported **2,719,036,048 bytes** of peak allocator memory. That is an MLX measurement, not the entire process or studio memory footprint. The weights loaded with strict parameter checks. Mido 1.3.3 independently read all 33 note-ons from the preview MIDI 1 file at 960 PPQ. Automated tests performed another real inference and checked source-request preservation, complete output notes, model identity, bounded invalid input, and cancellation before launch. The full repository suite now passes 62 tests without skips on this Mac, including real inference through the queue, acceptance, and complete archive restoration.

The output includes only newly generated notes. The adapter subtracts a multiset of the independently decoded prompt notes from the complete decoded result, rejecting a result if the tokenized prompt cannot be accounted for. The first generated onset is translated to zero for the worker result, while original model timestamps remain attached. The qualification preview places that answer after the untouched prompt with a 240-tick gap. Its two tracks contain 33 notes across five bars; the procedural prompt is locked.

Aria uses its own tokenizer timing and 7-bit velocity representation. The prompt is projected into that representation, and generated velocity is scaled for the OmniuSong project. That does not create measured MIDI 2 gesture precision. The studio keeps original high-resolution source data intact and uses a separate cropped piano projection for context. It explicitly records that expression is not used by this model.

## Compatibility fix

The first upstream MLX sampling attempt failed because `sample_mlx` leaves `max_kv_pos` unset, while the pinned model implementation adds one to that boundary. A small adapter subclass supplies the last input position explicitly and converts the RoPE offset to a scalar. The pinned upstream working trees remain unchanged. The worker verifies their revisions and refuses tracked local modifications. This fix is explicit in `workers/aria_worker.py` and the result metadata; loading alone was not counted as inference success.

## Reproduce and review

Requires Apple Silicon, Node 22+, uv, the Hugging Face CLI, and at least 5 GiB free for setup:

```sh
scripts/setup-aria.sh
node scripts/qualify-aria.mjs
```

The setup pins source revisions, dependency versions, checkpoint revision, size, and checksum. It disables implicit Hub credentials and keeps model storage inside the project. The worker accepts at most 256 prompt notes, 15 seconds of input, 1,024 prompt tokens, and 256 new tokens. The Node wrapper limits input/output sizes and applies cancellation plus a two-minute timeout. Each invocation checks the checkpoint hash. Prompt temporary files are removed afterward.

Review these artifacts:

- `verification/aria.json`: exact result, token trace, runtime identity, timing, memory observation, and artifact hashes.
- `verification/aria-project.json`: editable source prompt and separate AI answer.
- `verification/aria-continuation.mid`: MIDI 1 preview.
- `verification/aria-continuation.ump.json`: OmniuSong UMP JSON, not a standard MIDI Clip File.
- `verification/aria-continuation.wav`: preview synth rendering of the prompt followed by the AI answer. This is not neural piano audio synthesis.

Listening quality, human-played input, simultaneous microphone recording, prompt ownership comparisons, sustained live interaction, and controller input/output latency remain unqualified. The piano roll now has four-bar pages covering all 128 project bars and selectable pitch ranges covering notes 0–127. An actual browser edit at pitch 127 in bar five, save, undo, and reload passed.

## Studio continuation qualification

Select a pitched note track (including a locked one), open **Continue with Aria · local AI**, then choose a start bar, 1–4 source bars, an approximate answer length, and Seed. Brief, Standard, Extended, and Long map to 32, 64, 128, and 256 new-token limits respectively. Context must fit within the session and 15 seconds. Up to 256 overlapping notes are cropped at the selected boundaries only in the model request; source timing, 16-bit velocity, expression, locks, and original metadata remain unchanged. Tokenization has its own timing and 7-bit velocity limits.

The answer begins immediately after the selected source bars. The model's first generated onset is normalized to that location; original model timestamps remain in each note's estimate metadata. Audition plays the answer from its beginning without a silent wait for the source bars. Acceptance adds a separate track and extends the session if required, up to 128 bars. It may overlap other existing music. It does not shift other tracks or replace the source. A stale project revision prevents acceptance; a queued source revision change also prevents inference.

The browser **Aria studio qualification** session used a locked 12-note procedural prompt at 104 BPM. Seed 42 and seed 43 each produced 21 complete new notes from 64 tokens, with different musical results. The first was auditioned and discarded while the project stayed at two bars and 12 notes. The second was accepted, producing five bars and 33 notes. Its measured worker time was 2.247 seconds; the discarded run took 2.209 seconds. These are synthetic fixture observations, not quality or latency benchmarks.

Bar-five notes were visible. A temporary high-pitch edit saved and was undone, then reload retained the exact accepted music. Source-track comparison passed, and the complete project archive restored into an empty temporary store with identical tracks, model evidence, source scope, and five-bar length. Browser console errors were empty in this flow; 680- and 1440-pixel layouts were inspected.

- `verification/aria-studio.json`: both jobs, model/runtime evidence, decisions, source-preservation and backup receipt.
- `verification/aria-studio-source.json`: source project before generation.
- `verification/aria-studio-project.json`: accepted editable project after the edit/undo check.
- `verification/aria-studio.png`: accepted candidate and bar-five editor.

A bounded Magenta RealTime 2 Small experiment is next for the complementary audio-generation path, subject to resource, licensing, and runtime checks. Human-performed prompts, subjective listening comparisons, exact-duration control, and live accompaniment remain separate work.
