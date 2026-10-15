"""Bounded offline qualification of the official MRT2 Small exported MLX graph.

No download, microphone, external inference, or personal audio input is used.
MIDI conditioning is an explicitly lossy pitch/onset/sustain projection at 25 Hz.
"""
import contextlib
import hashlib
import importlib.metadata
import json
import math
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'workers/magenta-assets.json'

def integer(value, low, high):
    return type(value) is int and low <= value <= high

def validate_request(request):
    if not isinstance(request, dict):
        raise ValueError('Magenta needs a generation request')
    if not isinstance(request.get('prompt'), str) or not 1 <= len(request['prompt'].strip()) <= 800:
        raise ValueError('Use a style prompt with 1–800 characters')
    if not integer(request.get('seed'), 0, 0xffffffff) or not integer(request.get('durationSeconds'), 1, 8):
        raise ValueError('Choose a uint32 seed and 1–8 whole seconds')
    notes = request.get('notes', [])
    if not isinstance(notes, list) or len(notes) > 256:
        raise ValueError('Use at most 256 conditioning notes')
    tempo = request.get('tempo', 104)
    if type(tempo) not in (int, float) or not math.isfinite(tempo) or not 30 <= tempo <= 300:
        raise ValueError('Tempo must be 30–300 BPM')
    for note in notes:
        if not isinstance(note, dict) or not all(integer(note.get(key), lo, hi) for key, lo, hi in (
                ('pitch', 0, 127), ('startTick', 0, 1_000_000), ('durationTicks', 1, 1_000_000))):
            raise ValueError('Invalid Magenta conditioning note')
        if (note['startTick'] + note['durationTicks']) / 960 * 60 / tempo > request['durationSeconds']:
            raise ValueError('Conditioning notes must fit the requested audio duration')
    return request

def conditioning_frames(request):
    """Keep every nonempty note audible to the model for at least one frame."""
    frames = request['durationSeconds'] * 25
    if not request.get('notes'):
        return None
    result = [[0] * 128 for _ in range(frames)]
    for note in request['notes']:
        start = min(frames - 1, math.floor(note['startTick'] / 960 * 60 / request.get('tempo', 104) * 25))
        end = min(frames, max(start + 1, math.ceil((note['startTick'] + note['durationTicks']) / 960 * 60 / request.get('tempo', 104) * 25)))
        pitch = note['pitch']
        for frame in range(start, end):
            # An onset wins when repeated/overlapping notes share one pitch.
            result[frame][pitch] = max(result[frame][pitch], 2 if frame == start else 1)
    return result

def verify_assets():
    manifest = json.loads(MANIFEST.read_text())
    source = ROOT / '.runtime/magenta-source'
    for folder, revision in [(source, manifest['sourceRevision']),
            (source / 'magenta_rt/_vendor/sequence-layers', manifest['sequenceLayersRevision'])]:
        actual = subprocess.check_output(['git', '-C', str(folder), 'rev-parse', 'HEAD'], text=True).strip()
        if actual != revision or subprocess.run(['git', '-C', str(folder), 'diff', '--quiet', 'HEAD']).returncode:
            raise ValueError('Magenta source differs from the pinned qualification')
    for entry in manifest['files']:
        file = ROOT / '.runtime/magenta-model' / entry['path']
        if file.stat().st_size != entry['bytes']:
            raise ValueError('Unexpected Magenta asset size: ' + entry['path'])
        with file.open('rb') as handle:
            if hashlib.file_digest(handle, 'sha256').hexdigest() != entry['sha256']:
                raise ValueError('Magenta asset checksum failed: ' + entry['path'])
    return manifest

def main():
    started = time.monotonic()
    with open(sys.argv[1], 'rb') as handle:
        content = handle.read(128001)
    if len(content) > 128000:
        raise ValueError('Magenta request exceeds 128,000 bytes')
    request = validate_request(json.loads(content))
    manifest = verify_assets()
    frame_notes = conditioning_frames(request)
    with contextlib.redirect_stdout(sys.stderr):
        import mlx.core as mx
        import numpy as np
        import soundfile as sf
        from magenta_rt import MagentaRT2StdMlxfn, paths
        from magenta_rt.config import MUSICCOCA, PIANOROLL_WITH_ONSETS
        if not mx.metal.is_available():
            raise ValueError('Magenta MLX requires Apple Silicon Metal')
        paths.set_magenta_home(ROOT / '.runtime/magenta-model')
        load_started = time.monotonic()
        model = MagentaRT2StdMlxfn(size='mrt2_small', temperature=1.3, top_k=40,
            cfg_scales={'musiccoca': 3.0, 'notes': 1.0, 'drums': 1.0}, warmup_steps=5)
        loaded = time.monotonic()
        embedding = model.embed_style(request['prompt'], use_mapper=True, seed=request['seed'])
        # Tokenize once; the same style is used for every generated frame.
        style = model.tokenize_style(embedding).tolist()
        embedded = time.monotonic()
        mx.random.seed(request['seed'])
        state = None
        chunks, step_seconds, trace = [], [], []
        for frame in range(request['durationSeconds'] * 25):
            conditioning = {MUSICCOCA.key: style}
            if frame_notes is not None:
                conditioning[PIANOROLL_WITH_ONSETS.key] = frame_notes[frame]
                if frame == 0 or frame_notes[frame] != frame_notes[frame-1]:
                    trace.append({'frame': frame, 'onsets': [i for i, n in enumerate(frame_notes[frame]) if n == 2],
                        'sustains': [i for i, n in enumerate(frame_notes[frame]) if n == 1]})
            step_started = time.monotonic()
            waveform, state = model.generate(conditioning=conditioning, frames=1, state=state)
            step_seconds.append(time.monotonic() - step_started)
            chunks.append(waveform.samples)
        generation_seconds = time.monotonic() - embedded
        samples = np.concatenate(chunks, axis=0)
        if samples.shape != (request['durationSeconds'] * 48000, 2) or not np.isfinite(samples).all():
            raise ValueError('Magenta returned invalid audio samples')
        peak = float(np.max(np.abs(samples)))
        rms = float(np.sqrt(np.mean(samples.astype(np.float64) ** 2)))
        if rms <= 1e-6:
            raise ValueError('Magenta returned silent audio')
        sf.write(sys.argv[2], samples, 48000, subtype='PCM_16', format='WAV')
        peak_memory = mx.get_peak_memory()
    audio_bytes = Path(sys.argv[2]).read_bytes()
    print(json.dumps({'model': manifest['repository'] + '/mrt2_small', 'modelRevision': manifest['revision'],
        'sourceRevision': manifest['sourceRevision'], 'sequenceLayersRevision': manifest['sequenceLayersRevision'],
        'backend': 'mlx-exported-function', 'assetManifestSha256': hashlib.sha256(MANIFEST.read_bytes()).hexdigest(),
        'runtimeVersions': {name: importlib.metadata.version(name) for name in ['magenta-rt', 'mlx', 'numpy', 'ai-edge-litert', 'jax']},
        'python': sys.version.split()[0], 'seed': request['seed'], 'prompt': request['prompt'],
        'sampleRate': 48000, 'channels': 2, 'sampleFrames': samples.shape[0], 'durationSeconds': request['durationSeconds'],
        'generationFrames': len(step_seconds), 'elapsedSeconds': round(time.monotonic()-started, 3),
        'loadAndWarmupSeconds': round(loaded-load_started, 3), 'styleEmbeddingSeconds': round(embedded-loaded, 3),
        'generationSeconds': round(generation_seconds, 3), 'firstFrameSeconds': round(step_seconds[0], 5),
        'frameSecondsP50': round(float(np.median(step_seconds)), 5), 'frameSecondsP95': round(float(np.percentile(step_seconds, 95)), 5),
        'peakMLXBytes': peak_memory, 'peakAmplitude': peak, 'rmsAmplitude': rms,
        'clippedSampleCount': int(np.count_nonzero(np.abs(samples) >= 1)),
        'audioSha256': hashlib.sha256(audio_bytes).hexdigest(), 'audioBytes': len(audio_bytes),
        'conditioning': {'sourceNoteCount': len(request.get('notes', [])), 'frameRate': 25, 'trace': trace,
            'policy': 'Optional pitch/onset/sustain guidance at 25 Hz. Velocity and expression are not passed. No exact note adherence or MIDI 2 precision is claimed.'},
        'sampling': {'temperature': 1.3, 'topK': 40, 'styleGuidance': 3.0, 'noteGuidance': 1.0},
        'limits': 'Offline fixture. No microphone input, audio-context encoding, live device latency, sustained streaming, or listening quality qualification.'}))

if __name__ == '__main__':
    main()
