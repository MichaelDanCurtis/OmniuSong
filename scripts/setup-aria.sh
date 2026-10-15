#!/bin/sh
# Recreate the qualified Apple Silicon runtime. No account login or model gate.
set -eu
OMNIUSONG_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$OMNIUSONG_ROOT"
test "$(uname -s)" = Darwin && test "$(uname -m)" = arm64
command -v uv >/dev/null
command -v hf >/dev/null
mkdir -p .runtime
python3 - <<'PY'
import shutil
if shutil.disk_usage('.').free < 5 * 1024**3:
    raise SystemExit('Aria setup needs at least 5 GiB free for its bounded runtime and checkpoint')
PY
for OMNIUSONG_SOURCE in aria aria-utils; do
  case "$OMNIUSONG_SOURCE" in
    aria) OMNIUSONG_REV=c2f67bc90b49335543c0249fe00fb84a9337faf8 ;;
    aria-utils) OMNIUSONG_REV=7c815755c6889a784b6a1b3343e9e8daa3e7d128 ;;
  esac
  OMNIUSONG_PATH=".runtime/$OMNIUSONG_SOURCE-source"
  if test ! -d "$OMNIUSONG_PATH"; then
    git init "$OMNIUSONG_PATH"
    git -C "$OMNIUSONG_PATH" remote add origin "https://github.com/EleutherAI/$OMNIUSONG_SOURCE.git"
    git -C "$OMNIUSONG_PATH" fetch --depth 1 origin "$OMNIUSONG_REV"
    git -C "$OMNIUSONG_PATH" checkout --detach FETCH_HEAD
  fi
  test "$(git -C "$OMNIUSONG_PATH" rev-parse HEAD)" = "$OMNIUSONG_REV"
  git -C "$OMNIUSONG_PATH" diff --quiet
  git -C "$OMNIUSONG_PATH" diff --cached --quiet
done
if test ! -x .runtime/aria/bin/python; then uv venv --python 3.11 .runtime/aria; fi
uv pip install --link-mode copy --python .runtime/aria/bin/python -r workers/aria-resolved.txt
uv pip install --link-mode copy --python .runtime/aria/bin/python --no-deps -e .runtime/aria-source -e .runtime/aria-utils-source
HF_HUB_DISABLE_IMPLICIT_TOKEN=1 HF_HOME="$OMNIUSONG_ROOT/.runtime/huggingface" hf download loubb/aria-medium-base model-gen.safetensors README.md \
  --revision 5a1ef36c5007ee10fbe57d61d96725a7244bd2b2 --local-dir .runtime/aria-model --max-workers 1
.runtime/aria/bin/python - <<'PY'
import hashlib
from pathlib import Path
model = Path('.runtime/aria-model/model-gen.safetensors')
if model.stat().st_size != 2634170640:
    raise SystemExit('Unexpected Aria checkpoint size')
with model.open('rb') as handle:
    if hashlib.file_digest(handle, 'sha256').hexdigest() != '5e6b07ee2680f0044deea30da613d89b12e3e30d012d1812b388d6b8d8306c90':
        raise SystemExit('Aria checkpoint checksum failed')
print('Pinned Aria runtime installed. Run scripts/qualify-aria.mjs for actual inference.')
PY
