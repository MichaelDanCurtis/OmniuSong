#!/usr/bin/env python3
"""One bounded local Basic Pitch inference. JSON on stdout, diagnostics on stderr."""
import contextlib
import hashlib
import json
from pathlib import Path
import sys
import time
import uuid

started = time.monotonic()
with contextlib.redirect_stdout(sys.stderr):
    import basic_pitch
    from basic_pitch.inference import predict, Model
    import onnxruntime as ort
    import soundfile as sf

source = Path(sys.argv[1]).resolve()
tempo = float(sys.argv[2])
if not 30 <= tempo <= 300:
    raise ValueError('Invalid tempo')
info = sf.info(str(source))
if not 0 < info.duration <= 120:
    raise ValueError('Transcription requires a WAV no longer than 120 seconds')
model_path = Path(basic_pitch.__file__).parent / 'saved_models/icassp_2022/nmp.onnx'
model_hash = hashlib.sha256(model_path.read_bytes()).hexdigest()
with contextlib.redirect_stdout(sys.stderr):
    model = Model(model_path)
    options = ort.SessionOptions()
    options.intra_op_num_threads = 4
    options.inter_op_num_threads = 1
    model.model = ort.InferenceSession(str(model_path), sess_options=options, providers=['CPUExecutionProvider'])
    _, _, events = predict(source, model_or_model_path=model, midi_tempo=tempo, multiple_pitch_bends=True)

notes = []
for start, end, pitch, amplitude, bends in events:
    duration = max(1, round((float(end)-float(start))*tempo/60*960))
    raw = [int(v) for v in bends] if bends is not None else []
    points = []
    for index, bend in enumerate(raw):
        # Upstream contours use one-third-semitone bins. The raw estimates are
        # retained; the UMP lane below has an explicit two-semitone pitch range.
        normalized = max(-1.0, min(1.0, bend/3/2))
        value = round(0x80000000 + normalized*(0x7fffffff if normalized >= 0 else 0x80000000))
        points.append({'tick': round(index/max(1,len(raw)-1)*duration), 'value32': value})
    notes.append({'id':str(uuid.uuid4()),'pitch':int(pitch),'startTick':max(0,round(float(start)*tempo/60*960)),
        'durationTicks':duration,'velocity16':max(1,min(65535,round(float(amplitude)*65535))),
        'expression':[{'type':'pitch','rangeSemitones':2,'estimated':True,'points':points}] if points else [],
        'sourceEstimate':{'startSeconds':float(start),'endSeconds':float(end),'activationAmplitude':float(amplitude),
            'contourThirdSemitones':raw,'velocityIsEstimated':True}})
print(json.dumps({'model':'spotify/basic-pitch@0.4.0/icassp_2022/onnx','modelSha256':model_hash,
    'elapsedSeconds':round(time.monotonic()-started,3),'sourceDurationSeconds':info.duration,
    'notes':sorted(notes,key=lambda n:(n['startTick'],n['pitch']))}))
