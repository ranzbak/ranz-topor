#!/usr/bin/env bash
# TW7 design round: render the pattern proposal sketches (tools/tw7_sketch.py) on the VM (AGENT_RULES: no local rendering).
# Usage on the VM (hz.sh job ... -- bash tools/tw7_sketch_job.sh): writes $HZ_RUN_DIR/sketch/{*.png,numbers.json}.
set -euo pipefail
OUT="${HZ_RUN_DIR:?}/sketch"
PY=python3
for c in /root/venv/bin/python3 "${TOPOR_VENV:-/nonexistent}/bin/python3"; do
  if [ -x "$c" ] && "$c" -c "import matplotlib" 2>/dev/null; then PY="$c"; fi
done
"$PY" tools/tw7_sketch.py "$OUT" > "$HZ_RUN_DIR/sketch-numbers.json"
ls -l "$OUT"
