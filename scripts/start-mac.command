#!/bin/zsh
set -eu
OMNIUSONG_ROOT="${0:A:h:h}"
cd "$OMNIUSONG_ROOT"
mkdir -p .runtime
if ! curl --connect-timeout 1 --max-time 2 -fsS http://127.0.0.1:43178/api/status >/dev/null 2>&1; then
  nohup node src/server.mjs > .runtime/studio.log 2>&1 &
  print $! > .runtime/studio.pid
fi
for attempt in {1..30}; do
  if curl --connect-timeout 1 --max-time 2 -fsS http://127.0.0.1:43178/api/status >/dev/null 2>&1; then
    if [[ -d dist/OmniuSong.app ]]; then
      open dist/OmniuSong.app
    else
      open http://127.0.0.1:43178
    fi
    exit 0
  fi
  sleep 0.2
done
print 'The studio did not start. See .runtime/studio.log.'
exit 1
