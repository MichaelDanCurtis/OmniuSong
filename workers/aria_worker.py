"""Bounded, offline piano continuation using the pinned official Aria MLX runtime.

Input is a local JSON phrase. Output contains only newly generated notes, with
their first onset translated to zero. Original prompt timing is never replaced.
"""
import collections
import contextlib
import hashlib
import importlib.metadata
import json
import math
from pathlib import Path
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / '.runtime/aria-model/model-gen.safetensors'
MODEL_SHA = '5e6b07ee2680f0044deea30da613d89b12e3e30d012d1812b388d6b8d8306c90'
MODEL_REVISION = '5a1ef36c5007ee10fbe57d61d96725a7244bd2b2'
SOURCE_REVISION = 'c2f67bc90b49335543c0249fe00fb84a9337faf8'
UTILS_REVISION = '7c815755c6889a784b6a1b3343e9e8daa3e7d128'

def integer(value, low, high):
    return type(value) is int and low <= value <= high

def validate_request(request):
    if not isinstance(request, dict):
        raise ValueError('Aria needs a phrase request')
    tempo = request.get('tempo')
    if type(tempo) not in (int, float) or not math.isfinite(tempo) or not 30 <= tempo <= 300:
        raise ValueError('Tempo must be 30–300 BPM')
    if not integer(request.get('seed'), 0, 0xffffffff) or not integer(request.get('maxNewTokens'), 16, 256):
        raise ValueError('Invalid seed or token limit')
    notes = request.get('notes')
    if not isinstance(notes, list) or not 1 <= len(notes) <= 256:
        raise ValueError('Choose a piano phrase with 1–256 notes')
    for note in notes:
        if not isinstance(note, dict) or not all(integer(note.get(field), low, high) for field, low, high in (
            ('pitch', 0, 127), ('velocity16', 0, 65535), ('startTick', 0, 1_000_000), ('durationTicks', 1, 1_000_000))):
            raise ValueError('Invalid piano prompt note')
    duration = max(n['startTick'] + n['durationTicks'] for n in notes) / 960 * 60 / tempo
    if duration > 15:
        raise ValueError('Aria qualification prompts are limited to 15 seconds')
    return request

def note_key(message):
    data = message['data']
    return (message['channel'], data['pitch'], data['start'], data['end'], data['velocity'])

def new_messages(prompt_messages, result_messages):
    remaining = collections.Counter(note_key(message) for message in prompt_messages)
    generated = []
    for message in result_messages:
        key = note_key(message)
        if remaining[key]:
            remaining[key] -= 1
        else:
            generated.append(message)
    if any(remaining.values()):
        raise ValueError('Aria changed its tokenized prompt; cannot safely isolate the continuation')
    return generated

def main():
    started = time.monotonic()
    with open(sys.argv[1], 'rb') as handle:
        content = handle.read(128001)
    if len(content) > 128000:
        raise ValueError('Aria prompt exceeds its input limit')
    request = validate_request(json.loads(content))
    for folder, expected in [('aria-source', SOURCE_REVISION), ('aria-utils-source', UTILS_REVISION)]:
        actual = subprocess.check_output(['git', '-C', str(ROOT / '.runtime' / folder), 'rev-parse', 'HEAD'], text=True).strip()
        if actual != expected:
            raise ValueError('Aria source revision differs from the qualified runtime')
        if subprocess.run(['git', '-C', str(ROOT / '.runtime' / folder), 'diff', '--quiet', 'HEAD']).returncode:
            raise ValueError('Aria source has local modifications outside the qualified adapter')
    with MODEL.open('rb') as handle:
        if hashlib.file_digest(handle, 'sha256').hexdigest() != MODEL_SHA:
            raise ValueError('Aria checkpoint checksum differs from the qualified model')
    # Upstream sampling prints progress; the worker reserves stdout for its result.
    with contextlib.redirect_stdout(sys.stderr):
        import mlx.core as mx
        import torch
        from aria.config import load_model_config
        from aria.model import ModelConfig
        from aria.inference import get_inference_prompt
        from aria.inference.model_mlx import TransformerLM
        from aria.inference.sample_mlx import sample_batch
        from ariautils.midi import MidiDict
        from ariautils.tokenizer import AbsTokenizer

        if not mx.metal.is_available():
            raise ValueError('Aria MLX requires Apple Silicon Metal')
        torch.set_num_threads(2)
        torch.manual_seed(request['seed'])
        mx.random.seed(request['seed'])
        tokenizer = AbsTokenizer()
        midi = MidiDict(meta_msgs=[], tempo_msgs=[{'type': 'tempo', 'data': round(60_000_000/request['tempo']), 'tick': 0}],
            pedal_msgs=[], instrument_msgs=[{'type': 'instrument', 'data': 0, 'tick': 0, 'channel': 0}],
            note_msgs=[{'type': 'note', 'tick': n['startTick'], 'channel': 0, 'data': {
                'pitch': n['pitch'], 'start': n['startTick'], 'end': n['startTick']+n['durationTicks'],
                'velocity': max(1, n['velocity16'] >> 9)}} for n in request['notes']], ticks_per_beat=960, metadata={})
        prompt = get_inference_prompt(midi, tokenizer, 15000)
        if len(prompt) > 1024:
            raise ValueError('Tokenized Aria prompt exceeds 1,024 tokens')
        config = ModelConfig(**load_model_config('medium'))
        config.set_vocab_size(tokenizer.vocab_size)
        class QualifiedTransformerLM(TransformerLM):
            # Upstream sample_mlx omits max_kv_pos, but Transformer requires an
            # integer boundary. Use the last observed input position. Keep this
            # adapter shim explicit rather than editing the pinned upstream tree.
            def __call__(self, idxs, input_pos, offset, max_kv_pos=None, pad_idxs=None):
                boundary = int(input_pos[-1].item()) if max_kv_pos is None else max_kv_pos
                scalar_offset = int(offset.item()) if hasattr(offset, 'item') else offset
                return super().__call__(idxs=idxs, input_pos=input_pos, offset=scalar_offset, max_kv_pos=boundary, pad_idxs=pad_idxs)
        model = QualifiedTransformerLM(config)
        model.load_weights(str(MODEL), strict=True)
        mx.eval(model.parameters())
        generated_at = time.monotonic()
        tokens = sample_batch(model=model, tokenizer=tokenizer, prompt=prompt, num_variations=1,
            max_new_tokens=request['maxNewTokens'], temp=.95, min_p=.035)[0]
        generated_seconds = time.monotonic() - generated_at
        decoded_prompt = tokenizer.detokenize(prompt)
        decoded = tokenizer.detokenize(tokens)
        messages = new_messages(decoded_prompt.note_msgs, decoded.note_msgs)
        if not messages:
            raise ValueError('Aria returned no complete continuation notes in the token budget')
        first_ms = min(decoded.tick_to_ms(m['data']['start']) for m in messages)
        notes = []
        for message in messages:
            data = message['data']
            start_ms = decoded.tick_to_ms(data['start'])
            end_ms = decoded.tick_to_ms(data['end'])
            velocity = data['velocity']
            notes.append({'id': str(uuid.uuid4()), 'pitch': data['pitch'],
                'startTick': max(0, round((start_ms-first_ms)/1000*request['tempo']/60*960)),
                'durationTicks': max(1, round((end_ms-start_ms)/1000*request['tempo']/60*960)),
                'velocity16': (velocity << 9) + (((velocity & 63) << 3) + ((velocity & 63) >> 3) if velocity > 64 else 0),
                'expression': [], 'modelEstimate': {'startMilliseconds': start_ms, 'endMilliseconds': end_ms, 'velocity7': velocity}})
        peak_memory = mx.get_peak_memory()
    print(json.dumps({'model': 'loubb/aria-medium-base/model-gen.safetensors', 'modelRevision': MODEL_REVISION,
        'modelSha256': MODEL_SHA, 'sourceRevision': SOURCE_REVISION, 'utilsRevision': UTILS_REVISION,
        'backend': 'mlx', 'runtimeVersions': {'python': sys.version.split()[0], 'mlx': importlib.metadata.version('mlx'), 'torch': importlib.metadata.version('torch'), 'numpy': importlib.metadata.version('numpy')},
        'runtimeCompatibility': 'Explicit attention-cache boundary and scalar RoPE offset in adapter subclass; upstream source unchanged',
        'elapsedSeconds': round(time.monotonic()-started, 3), 'generationSeconds': round(generated_seconds, 3),
        'peakMLXBytes': peak_memory, 'seed': request['seed'], 'promptTokens': len(prompt), 'generatedTokens': len(tokens)-len(prompt),
        'decodedPromptNotes': len(decoded_prompt.note_msgs), 'firstGeneratedOnsetMilliseconds': first_ms,
        'timingPolicy': 'New notes only. First generated onset translated to zero; original model times retained. Prompt velocity projected to 7 bits.',
        'notes': sorted(notes, key=lambda n: (n['startTick'], n['pitch'])), 'tokenTrace': tokens}))

if __name__ == '__main__':
    main()
