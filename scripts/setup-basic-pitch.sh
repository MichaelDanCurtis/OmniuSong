#!/bin/sh
# Install the ONNX-only runtime in this project's isolated environment.
set -eu
OMNIUSONG_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$OMNIUSONG_ROOT"
uv venv --python 3.11 .runtime/basic-pitch
uv pip install --python .runtime/basic-pitch/bin/python --no-deps basic-pitch==0.4.0
uv pip install --python .runtime/basic-pitch/bin/python numpy==1.26.4 librosa==0.10.2.post1 mir_eval==0.8.2 pretty_midi==0.2.10 resampy==0.4.2 onnxruntime==1.20.1 scipy==1.16.3 scikit-learn==1.6.1 setuptools==80.9.0 typing_extensions
