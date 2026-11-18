#!/usr/bin/env bash
# exist-term r3 final check (Hetzner): the targeted tests (xt3_fix.sh), then clippy as CI (xt3_clippy.sh).
set -u
bash tools/xt3_fix.sh
bash tools/xt3_clippy.sh
