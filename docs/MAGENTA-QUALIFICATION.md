# Magenta RealTime 2 Small qualification

October 15, 2026. Actual local audio generation now runs through a bounded worker on this Apple Silicon Mac. Two four-second renders and a saved studio preview are available. General studio generation controls and live accompaniment are still pending.

## Identity and installation

This uses the unmodified [official Python/MLX implementation](https://github.com/magenta/magenta-realtime/tree/694a545e4ba0b88bf1150137b129582166d3e07f) and the Small exported graph from [Google's model repository](https://huggingface.co/google/magenta-realtime-2/tree/010aa0dcb0dfd27b24f0ad07b4dad63e8f9521cc). The [model card](https://huggingface.co/google/magenta-realtime-2/blob/010aa0dcb0dfd27b24f0ad07b4dad63e8f9521cc/README.md) identifies Google DeepMind, Apache 2.0 code, CC BY 4.0 weights, and additional usage terms. Copyright 2026 Google LLC. Model assets remain outside the source repository; this qualification does not bundle or redistribute them.

| Item | Qualified identity |
| --- | --- |
| Model | `google/magenta-realtime-2`, `mrt2_small` exported MLX function |
| Model revision | `010aa0dcb0dfd27b24f0ad07b4dad63e8f9521cc` |
| Official code | `694a545e4ba0b88bf1150137b129582166d3e07f` |
| Sequence Layers submodule | `c85a637a1afae8f4f2d4edb458f2111b5f44ae52` |
| Python / Magenta / MLX | 3.12.11 / 2.0.3 / 0.31.2 |
| LiteRT / NumPy | 2.1.5 / 2.3.5 |
| Locations | `.runtime/magenta`, `.runtime/magenta-source`, `.runtime/magenta-model` |

Hub metadata reported the model ungated. Setup disabled implicit Hub credentials and used no authentication or gate acceptance. Before installation, this host reported 128 GiB memory, 81% system memory free, and approximately 741 GiB available on the external disk.

Only six model assets plus the model card were fetched: the Small graph/state and the text encoder, mapper, vector quantizer, and vocabulary. The six assets total **1,042,112,092 bytes**. Exact file sizes and SHA256 hashes are in `workers/magenta-assets.json`. The isolated environment occupies approximately 909 MiB, model storage 994 MiB, and pinned source 9.5 MiB. The original raw checkpoint, Base model, audio style encoder, and separate codec files were not downloaded. Audio-context conditioning is consequently not available in this qualification.

Dependencies use the pinned upstream lock, including JAX dependencies pulled in by that package. The executed generation path is the exported MLX graph; text-style embedding uses LiteRT. This is not a JAX generation benchmark. Setup was rerun successfully against the cached files without another model download.

## Actual generation

Both runs use the same synthetic brief, seed 42, and four-second duration. The style is warm electric piano, brushed drums, relaxed instrumental jazz, and a text request for 104 BPM. The second run additionally receives C4, E4, G4, and C5 note guidance. No personal performance or recording was used.

| Measurement | Style only | Same style with note guidance |
| --- | --- | --- |
| Audio | 48 kHz, two channels, PCM 16 WAV | Same |
| Sample frames / model frames | 192,000 / 100 | Same |
| Worker total | 3.051 s | 2.999 s |
| Generation phase | 1.127 s | 1.131 s |
| Per-frame median / 95th percentile | 11.22 / 11.84 ms | 11.22 / 12.02 ms |
| Peak MLX allocator | 534,312,136 bytes | 533,955,882 bytes |
| RMS / peak amplitude | 0.0948 / 0.4632 | 0.0375 / 0.2194 |
| Clipped samples | 0 | 0 |

The two audio checksums differ. This establishes that conditioning changes this fixture's output; it does not establish exact note adherence or musical quality. These are short offline observations with a warm filesystem. MLX allocator measurements exclude other process allocations. Generation faster than the clip duration does not establish live device latency, dropout resistance, sustained streaming, or simultaneous recording performance.

## Human-control projection and preservation

The adapter accepts up to 256 source notes and projects their pitch/onset/sustain states onto 25 Hz frames. Every nonempty note receives at least one frame; repeated onsets win over sustain when the same pitch overlaps. Events within one 40 ms frame cannot retain their distinct timing. Velocity, release velocity, expression, and MIDI 2 gestures are not passed to this model. The original request is unchanged. The saved preview retains the exact four source notes, including their 16-bit velocities, as a locked, muted procedural guide.

The preview contains two separate generated audio tracks. Only the note-guided render is unmuted in arrangement playback. Both raw four-second WAVs have independent players and downloads labeled **Download generated WAV**. Generated audio has its own project origin and is not labeled as a microphone take. The source guide and both audio files survived complete archive restoration into an empty store, including byte-for-byte asset comparison.

## Worker and verification

`workers/magenta_worker.py` validates input before loading models, checks both official source revisions and all six asset checksums, and generates no more than eight whole seconds. It never downloads from the Hub or accepts external audio. The Node wrapper uses private temporary input/output paths, an isolated environment, a two-minute timeout, cancellation, bounded output, and independent PCM WAV/header/size/checksum checks. Temporary files are removed after success or failure. Installation presence and studio controls are reported separately by `/api/status`.

Automated checks cover short/repeated/sustained note projection, invalid pitches and budgets, cancellation before launch and during worker startup, actual note-conditioned inference, source preservation, generated-origin metadata, and complete backups. The complete suite passes **65 tests without skips** on this Mac. Browser checks verified session selection, generated-audio labels, transport start/stop, both four-second media durations, raw-player keyboard playback, and no captured console errors. Keyboard shortcuts now leave media controls and focused buttons' Space behavior intact.

## Reproduce and listen

Requires Apple Silicon, uv, the Hugging Face CLI, and at least 4 GiB available:

```sh
scripts/setup-magenta.sh
node scripts/qualify-magenta.mjs
```

- `verification/magenta-prompt.json`: synthetic source and brief.
- `verification/magenta.json`: model manifest, both generation receipts, source-preservation and backup evidence, browser checks.
- `verification/magenta-style.wav`: actual model audio without note guidance.
- `verification/magenta-notes.wav`: actual model audio with the procedural note guide.
- `verification/magenta-project.json`: editable preview project and model provenance.
- `verification/magenta-studio.png`: saved studio preview.

The **Magenta Small · local audio preview** session is ready for listening. Listening quality, exact tempo/note adherence, human-played guidance, microphones, streaming interaction, and audio-context conditioning remain unqualified. Next, put this bounded audio worker behind the existing candidate queue with clear scope, source retention, and audition/accept/discard; then evaluate sustained streaming as a separate stage.
