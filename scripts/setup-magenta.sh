#!/bin/sh
# Recreate the isolated MRT2 Small qualification; no account or audio upload.
set -eu
OMNIUSONG_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$OMNIUSONG_ROOT"
test "$(uname -s)" = Darwin && test "$(uname -m)" = arm64
command -v uv >/dev/null
command -v hf >/dev/null
mkdir -p .runtime
python3 - <<'PY'
import shutil
if shutil.disk_usage('.').free < 4 * 1024**3:
    raise SystemExit('Magenta setup needs at least 4 GiB free for its bounded runtime and selected assets')
PY
OMNIUSONG_SOURCE=.runtime/magenta-source
OMNIUSONG_REV=694a545e4ba0b88bf1150137b129582166d3e07f
if test ! -d "$OMNIUSONG_SOURCE"; then
  git init "$OMNIUSONG_SOURCE"
  git -C "$OMNIUSONG_SOURCE" remote add origin https://github.com/magenta/magenta-realtime.git
  git -C "$OMNIUSONG_SOURCE" fetch --depth 1 origin "$OMNIUSONG_REV"
  git -C "$OMNIUSONG_SOURCE" checkout --detach FETCH_HEAD
fi
test "$(git -C "$OMNIUSONG_SOURCE" rev-parse HEAD)" = "$OMNIUSONG_REV"
git -C "$OMNIUSONG_SOURCE" diff --quiet HEAD
git -C "$OMNIUSONG_SOURCE" submodule update --init --depth 1 magenta_rt/_vendor/sequence-layers
test "$(git -C "$OMNIUSONG_SOURCE/magenta_rt/_vendor/sequence-layers" rev-parse HEAD)" = c85a637a1afae8f4f2d4edb458f2111b5f44ae52
git -C "$OMNIUSONG_SOURCE/magenta_rt/_vendor/sequence-layers" diff --quiet HEAD
UV_PROJECT_ENVIRONMENT="$OMNIUSONG_ROOT/.runtime/magenta" UV_LINK_MODE=copy uv sync --project "$OMNIUSONG_SOURCE" --python 3.12.11 --extra mlx --no-dev --frozen
HF_HUB_DISABLE_IMPLICIT_TOKEN=1 HF_TOKEN='' HF_HOME="$OMNIUSONG_ROOT/.runtime/huggingface" python3 - <<'PY'
import hashlib
import json
from pathlib import Path
import subprocess
manifest=json.loads(Path('workers/magenta-assets.json').read_text())
assert len(manifest['files']) == 6 and manifest['totalBytes'] < 1_100_000_000
subprocess.run(['hf','download',manifest['repository'],*[f['path'] for f in manifest['files']],'README.md',
    '--revision',manifest['revision'],'--local-dir','.runtime/magenta-model','--max-workers','1'],check=True)
for entry in manifest['files']:
    file=Path('.runtime/magenta-model')/entry['path']
    if file.stat().st_size != entry['bytes']:
        raise SystemExit('Magenta asset size mismatch: '+entry['path'])
    with file.open('rb') as handle:
        if hashlib.file_digest(handle,'sha256').hexdigest()!=entry['sha256']:
            raise SystemExit('Magenta checksum failed: '+entry['path'])
print('Pinned Magenta Small runtime installed. Run node scripts/qualify-magenta.mjs for actual inference.')
PY
