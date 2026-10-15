# Aria MLX qualification

October 15, 2026. This checkout now runs an actual local symbolic piano model alongside Basic Pitch transcription. Aria generation is callable through the bounded worker API; the studio's general continuation controls are still pending. The saved **Aria piano · local model preview** session lets you play and inspect this qualification result today.

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

MLX reported **2,719,036,048 bytes** of peak allocator memory. That is an MLX measurement, not the entire process or studio memory footprint. The weights loaded with strict parameter checks. Mido 1.3.3 independently read all 33 note-ons from the preview MIDI 1 file at 960 PPQ. Automated tests performed another real inference and checked source-request preservation, complete output notes, model identity, bounded invalid input, and cancellation before launch. The full repository suite passed 56 tests without skips on this Mac.

The output includes only newly generated notes. The adapter subtracts a multiset of the independently decoded prompt notes from the complete decoded result, rejecting a result if the tokenized prompt cannot be accounted for. The first generated onset is translated to zero for the worker result, while original model timestamps remain attached. The qualification preview places that answer after the untouched prompt with a 240-tick gap. Its two tracks contain 33 notes across five bars; the procedural prompt is locked.

Aria uses its own tokenizer timing and 7-bit velocity representation. The prompt is projected into that representation, and generated velocity is scaled for the OmniuSong project. That does not create measured MIDI 2 gesture precision. Original high-resolution source data must remain in the project when studio integration is added.

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

Listening quality, human-played input, simultaneous microphone recording, prompt ownership comparisons, sustained live interaction, and controller input/output latency remain unqualified. The current piano-roll view also has a fixed four-bar editing window and pitch range; transport and arrangement can play the complete five-bar preview. Timeline navigation and phrase placement must be completed before exposing a general continuation workflow.

The next slice should put this worker behind the existing candidate queue with explicit source scope and response length, safe phrase placement, full timeline review, and audition/accept/discard. A bounded Magenta RealTime 2 Small experiment can then evaluate the complementary audio-generation path.
