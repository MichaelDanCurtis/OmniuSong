# OmniuSong model research and recommendations

Research began October 14, 2026. Primary sources were checked during the October 14–15 session. This catalog contains 36 model, model-family, and supporting-tool candidates. It is a design shortlist; installation and musical qualification are separate.

## Recommended starting rig

Build around complementary musical roles. MiniMax Music 3 and ACE-Step cover song sketches and audio transformations. Aria covers expressive piano continuation. Basic Pitch and MuScriptor cover different transcription needs. DDSP and RAVE explore playable neural timbres. DiffSinger supplies a score-driven singing path. MuLaCover connects human melody and chord control to generated audio. Magenta RealTime 2 is the first local live-accompaniment experiment.

The Mac-first order is Basic Pitch, Aria MLX, and a bounded Magenta RealTime 2 Small experiment, followed by a qualified song-generation worker. Keep the real-time instrument usable while an external worker handles large renders. MiniMax Music 3 remains a priority, with its runtime and distribution terms resolved before bundling.

## Findings that change the design

- **MiniMax Music 3 is the relevant current family.** It has open weights and generates songs from lyrics and a music description. Its published inference is non-streaming. The hosted music and lyric APIs stopped accepting new paid users on August 20, 2026, so a new product should not depend on that API being available. [Model card](https://huggingface.co/MiniMaxAI/MiniMax-Music3), [API notice](https://platform.minimax.io/docs/guides/music-generation).
- **A score can be an important control surface.** MuLaCover accepts melody/chord MIDI or reference music. YuE2 uses symbolic planning and score editing; its documented starting configuration targets Linux/NVIDIA. These deserve experiments for preserving a human tune while changing an arrangement. [MuLaCover](https://github.com/HeartMuLa/MuLaCover), [YuE2](https://github.com/multimodal-art-projection/YuE).
- **There are credible local Mac paths.** Aria publishes an MLX backend for symbolic piano continuation. Magenta RealTime 2 documents Apple Silicon support and different model sizes. Those are unusually relevant to a keyboard-first workstation. Their runtime claims still need measurement under simultaneous recording. [Aria](https://github.com/EleutherAI/aria), [Magenta RealTime 2](https://github.com/magenta/magenta-realtime).
- **Sound generation and instrument building are separate.** Stable Audio 3 offers a Mac optimization path and LoRA tooling. DDSP and RAVE address learned synthesis and timbre control. A generated sound still needs a playable mapping, envelopes, and musical testing. [Stable Audio 3](https://github.com/Stability-AI/stable-audio-3), [DDSP](https://github.com/magenta/ddsp), [RAVE](https://github.com/acids-ircam/RAVE).
- **Audio-to-MIDI models are not MIDI 2 engines.** Basic Pitch includes pitch bends and works best on an individual instrument. MuScriptor targets multi-instrument transcription. Preserve model estimates, confidence where available, and source audio before translating supported data into UMP. [Basic Pitch](https://github.com/spotify/basic-pitch), [MuScriptor](https://github.com/muscriptor/muscriptor).

## Access and license observations

These observations determine integration work; they are not a completed distribution review.

| Candidate | Code or model observation | Consequence |
| --- | --- | --- |
| MiniMax Music 3 | Public model metadata has no license label in the captured response | Resolve the checkpoint's actual terms; public download is not a license conclusion |
| ACE-Step 1.5 | Upstream repository identifies MIT licensing | Pin and inspect all chosen model and dependency artifacts |
| Stable Audio 3 Small Music | Hugging Face reports an automatic gate and an “other” license | Account acceptance and the actual checkpoint terms remain required |
| HeartMuLa | Upstream states Apache 2.0 for repository and related weights | Verify the exact selected artifact and its provenance |
| MuLaCover | Separate code and model license files | Read both before packaging weights |
| AudioCraft MusicGen | MIT code, CC BY NC 4.0 weights | Keep the default commercial distribution path separate |
| Aria | Upstream describes Apache 2.0 models and tooling | Pin the exact generative checkpoint |
| YourMT3 and Seed-VC | GPL code is identified upstream | Evaluate integration and distribution requirements before embedding |
| Demucs | Original upstream repository is archived | Pin a maintained implementation or own the compatibility work |
| Hosted models | Provider access and current terms govern use | Add an explicit provider budget and verify actual account access |

Machine-readable source metadata, including queried model revisions, is retained in `source-metadata.json`. A source checked today can still describe a feature that is not usable on the current machine; runtime qualification closes that gap.

## Candidate catalog

Each source is linked directly. “Keep” means preserve it in the proposed rig, not install it or declare it musically superior. “Explore” means run a focused comparison. “Skip” means exclude it from the first default stack while retaining the research reference.

### Full songs

**[MiniMax Music 3](https://huggingface.co/MiniMaxAI/MiniMax-Music3) · Keep**

Complete songs conditioned on lyrics and a music description. Controls and inputs: Lyrics, arrangement descriptions, long-form audio. Open weights; non-streaming. Mac runtime must be qualified. Hosted music API access is restricted for new users.

**[ACE-Step 1.5](https://github.com/ace-step/ACE-Step-1.5) · Keep**

Song generation and editing in an extensible local model family. Controls and inputs: Lyrics, reference audio, covers, repainting, LoRA. Official Mac support is documented. Measure quality and memory on this Mac before selecting defaults.

**[Stable Audio 3 Medium](https://github.com/Stability-AI/stable-audio-3) · Explore**

A larger audio and music generation candidate for a GPU worker. Controls and inputs: Text, reference audio, LoRA. The main runtime targets CUDA. Keep outside the Mac audio callback.

**[HeartMuLa](https://github.com/HeartMuLa/heartlib) · Explore**

Lyrics-and-tags music generation with a family of music understanding tools. Controls and inputs: Lyrics, tags, multilingual singing. Use the published 3B checkpoint as the comparison baseline; internal-model claims are not shipping capability.

**[LeVo 2 SongGeneration 2](https://github.com/tencent-ailab/SongGeneration) · Explore**

A comparison candidate for full songs and lyric adherence. Controls and inputs: Text and audio prompts. Upstream repository could not be read during this research pass. Verify release files, checkpoint license, and source availability before integration.

**[DiffRhythm 2](https://aslp-lab.github.io/DiffRhythm2.github.io/) · Explore**

Diffusion-based song generation with vocals and accompaniment. Controls and inputs: Lyrics, style and reference conditioning. Research candidate; qualify runtime and model terms separately.

**[Lyria 3.5](https://ai.google.dev/gemini-api/docs/music-generation) · Explore**

Hosted song generation with vocals, song structure, and timed lyrics. Controls and inputs: Text, image, song structure. Requires an enabled Gemini API account and a spending limit; no cloud calls are enabled in this build.

**[Eleven Music v2.5](https://elevenlabs.io/docs/overview/capabilities/music) · Explore**

Hosted music generation with composition plans, audio references, and inpainting. Controls and inputs: Composition plan, lyrics, reference audio, section edits. Commercial API and account terms apply. Finetunes are a separate offering.

**[MusicGen](https://github.com/facebookresearch/audiocraft) · Skip**

An established text-conditioned music generation baseline. Controls and inputs: Text and continuation. AudioCraft weights use CC BY NC 4.0. Keep research use separate from commercial product distribution.

### Instruments and samples

**[Stable Audio 3 Small Music](https://github.com/Stability-AI/stable-audio-3) · Keep**

Lightweight music generation with a documented Apple Silicon optimization path. Controls and inputs: Text, audio conditioning, stackable LoRA. Code and model licenses are separate. A generated sample needs mapping and loop editing to become an instrument.

**[Stable Audio 3 Small SFX](https://huggingface.co/stabilityai/stable-audio-3-small-sfx) · Explore**

Generate percussion textures, objects, foley, and unusual sound material. Controls and inputs: Text, sample design, LoRA. Sound generation is not a multisampled playable instrument. Verify checkpoint terms.

**[RoyalCities sample workflows](https://github.com/RoyalCities/RC-stable-audio-tools) · Explore**

A workflow toolkit for generating production-oriented sample material. Controls and inputs: Sample prompts, timbre experiments, fine-tuning. This is a toolkit and checkpoint ecosystem, not one model. Qualify each selected checkpoint and license.

**[AudioLDM 2](https://github.com/haoheliu/AudioLDM2) · Explore**

Text-to-audio and music generation with editing and inpainting research paths. Controls and inputs: Text, audio edits, inpainting. Lower-priority research baseline; checkpoint licensing and quality need qualification.

**[TangoFlux](https://github.com/declare-lab/TangoFlux) · Explore**

Fast text-to-audio synthesis for textures and sound design. Controls and inputs: Text, audio prompt design. Evaluate isolated instruments and transients rather than assuming full-song performance.

### Score to audio

**[MuLaCover](https://github.com/HeartMuLa/MuLaCover) · Keep**

Generate covers or new arrangements from reference audio or melody and chord MIDI. Controls and inputs: Melody MIDI, chord MIDI, lyrics, style. Strong candidate for human score control. Code and model have separate license files.

**[YuE2](https://github.com/multimodal-art-projection/YuE) · Explore**

A song generator with symbolic planning, covers, and score editing. Controls and inputs: Lyrics, style, symbolic score, editing. Documented starting configuration is Linux with NVIDIA BF16 and 24 GB VRAM. Use a worker rather than assuming native Mac operation.

**[MusicGen Melody](https://github.com/facebookresearch/audiocraft) · Skip**

Generate music guided by the melody of a reference recording. Controls and inputs: Text and melody audio. Melody audio is different from editable MIDI conditioning. AudioCraft model weights are noncommercial.

**[JASCO](https://github.com/facebookresearch/audiocraft) · Explore**

Joint audio and symbolic conditioning for controllable generation. Controls and inputs: Chord, melody and drum controls. Research comparison; inspect the specific checkpoint terms and version before use.

### Live accompaniment

**[Lyria RealTime](https://ai.google.dev/gemini-api/docs/realtime-music-generation) · Keep**

Continuously steer instrumental music through a live streaming connection. Controls and inputs: Weighted prompts, tempo, density, brightness. Experimental hosted model. Continuous audio output does not imply live microphone conditioning.

**[Magenta RealTime 2](https://github.com/magenta/magenta-realtime) · Keep**

Open-weight live music generation with Apple Silicon model variants. Controls and inputs: Live audio and text steering, local streaming. Official small-model Mac support is promising; measure latency under simultaneous playing and recording.

### Neural instruments

**[DDSP](https://github.com/magenta/ddsp) · Keep**

Learn instrument timbre using differentiable synthesis components. Controls and inputs: Pitch, loudness, learned timbre. Useful for performance-controlled instruments; training data and a playable wrapper are still needed.

**[RAVE](https://github.com/acids-ircam/RAVE) · Keep**

Realtime neural audio synthesis and timbre exploration with a learned latent space. Controls and inputs: Audio, latent controls, trained timbres. Experimental instrument direction; inspect model weights and runtime latency for each patch.

### Singing and choir

**[DiffSinger](https://github.com/openvpi/DiffSinger) · Keep**

Score-driven singing synthesis with explicit phoneme and musical controls. Controls and inputs: Notes, phonemes, pitch and expression. Engine code, vocoder, and each voicebank can have different terms. Choir arrangement is an OmniuSong layer.

**[Seed-VC](https://github.com/Plachtaa/seed-vc) · Explore**

Voice and singing conversion guided by a reference performance. Controls and inputs: Reference voice, source singing, pitch controls. GPL code and checkpoint terms require review. Use voices you are authorized to use. Conversion does not write harmony.

### Lyrics and understanding

**[HeartTranscriptor](https://github.com/HeartMuLa/heartlib) · Explore**

A lyrics-transcription model from the HeartMuLa family. Controls and inputs: Sung audio to words. Singing transcription differs from ordinary speech recognition. Measure word accuracy before lyric alignment.

**[Qwen family through Ollama](https://github.com/QwenLM/Qwen3) · Keep**

Use an installed instruction model to draft and revise original lyrics. Controls and inputs: Theme, rhyme, syllable targets, structured lyrics. A general language model needs a song-specific evaluation. Model choice is explicit; the app never downloads one silently.

**[Other local instruction models](https://github.com/ollama/ollama) · Explore**

Swap lyric and musical-planning models through a local Ollama adapter. Controls and inputs: Model selection, prompts, structured output. Ollama is a runtime, not a music model. Each installed model has its own license and quality profile.

### Audio to MIDI

**[Spotify Basic Pitch](https://github.com/spotify/basic-pitch) · Keep**

Lightweight polyphonic note transcription with pitch bends, including voice recordings. Controls and inputs: One instrument or isolated vocal to notes. Best on one instrument at a time. Outputs MIDI 1; preserving estimated curves for MIDI 2 needs an adapter.

**[MuScriptor](https://github.com/muscriptor/muscriptor) · Keep**

Multi-instrument transcription developed by Kyutai and Mirelo. Controls and inputs: Mixed music to separate instrumental note parts. A high-priority contemporary transcription candidate. Validate every checkpoint and its terms.

**[Magenta MT3](https://github.com/magenta/mt3) · Explore**

Multi-task, multi-instrument transcription research baseline. Controls and inputs: Mixed audio to instrument-tagged notes. The original T5X stack has substantial environment requirements. Keep it in an isolated worker.

**[YourMT3 Plus](https://github.com/mimbres/YourMT3) · Explore**

A multi-task, multi-track transcription candidate. Controls and inputs: Multi-instrument audio to MIDI. GPL code; current executable code and checkpoint provenance must be pinned, rather than inferred from a demo.

**[Aria AMT](https://github.com/EleutherAI/aria-amt) · Explore**

Piano transcription candidate for expressive piano recordings. Controls and inputs: Piano audio to symbolic performance. Piano-specific. Keep alongside the general multi-instrument path.

### Symbolic composition

**[Aria](https://github.com/EleutherAI/aria) · Keep**

Expressive piano MIDI continuation with an official MLX implementation. Controls and inputs: Human MIDI phrase, continuation, expressive timing. A particularly good Mac-first candidate. MIDI 2 transport is an OmniuSong responsibility.

**[Anticipatory Music Transformer](https://github.com/jthickstun/anticipation) · Explore**

Compose symbolic music around constraints and existing musical events. Controls and inputs: MIDI events, future constraints, infilling. Research integration requires adapting its event vocabulary to the project note model.

### Separation

**[HT Demucs](https://github.com/facebookresearch/demucs) · Keep**

Separate music into vocals, drums, bass and other parts. Controls and inputs: Mix to stems. The original Meta repository is archived. Pin the implementation and avoid presenting maintenance as active.

**[SAM Audio](https://github.com/facebookresearch/sam-audio) · Explore**

Isolate audio targets using text, spans, or visual prompts. Controls and inputs: A target sound description, audio, optional video. General source separation is useful for unusual material; code and model licensing need inspection.

## Experiments worth getting a little strange with

| Experiment | Building blocks | What the human controls |
| --- | --- | --- |
| Your voice becomes an impossible orchestra | Vocal transcription, DDSP or a multisample instrument | Melody, breath, register, timbre morph, dynamics |
| A choir made from one original chorus | Lead-sheet review, harmony arranger, singing synthesis, optional authorized conversion | Voicings, singer count, lyrics, timing spread, identity blend |
| A pianist that answers your hands | Aria continuation, phrase handoff, audition queue | Which hand or register, motif locks, response length |
| A sound garden that becomes a keyboard | Generated samples, RAVE latent exploration, sample mapper | Timbre location, velocity behavior, envelopes, playable range |
| A band steered by pads and knobs | Qualified realtime generation and controller maps | Scene changes, density, prompt weights, arrangement capture |
| Rewrite the world around a melody | Human MIDI, MuLaCover or a qualified score-conditioned model | Exact retained tune, editable chords, instrumentation, style |
| Regenerate only the weak bar | Clip scope, hard locks, inpainting or score continuation | Protected notes, transition length, amount of variation |
| A tournament for musical taste | Shared brief, model adapters, blind audition, saved decisions | Rubric, favored take, explanation, reuse of chosen settings |

These are OmniuSong compositions of capabilities. A model does not acquire them simply because it appears in this catalog.

## Model comparison method

Use the same eight briefs for every applicable adapter: a piano phrase continuation, a hummed melody, a dense instrumental mix, an unusual percussion sample, a clear sung chorus, an eight-bar section edit, a melody-locked rearrangement, and a live steering performance. Use original or appropriately authorized inputs.

Record the exact checkpoint and adapter version, parameters, seed where supported, machine, runtime, elapsed time, memory observations, output sample format, and errors. Score musicality, controllability, editability, intelligibility, continuity, instrument quality, responsiveness, and integration effort separately. Do not substitute a vendor's self-reported comparison for OmniuSong's listening results.

Keep the generated audio, symbolic output, original input, and listening notes together. Measure real-time factors separately from time to first usable sound. Test cancellation and resource contention alongside quality. A fast standalone run is not proof that a model can share the machine with recording.

## Paths deliberately left open

Suno and Udio remain useful product references and potential manual audio-import sources. This plan does not rely on an unofficial API, browser automation around a consumer service, or third-party endpoints that claim to represent them. Add an official integration if an accessible, documented offering fits the product later.

Ordinary software instruments, sample libraries, effects, and human recordings stay first-class. The substrate should make those useful with or without an AI provider.
