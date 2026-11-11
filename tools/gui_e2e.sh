#!/usr/bin/env bash
# Live end-to-end test of the Topor plugin in a real KiCad 10 on a nested X display (Xephyr), with a separate
# KiCad home under <out_dir>/home: the user's KiCad, its config and plugin folders are never touched.
#   tools/gui_e2e.sh <board|pcb> <out_dir> [--display :6] [--wheel WHEEL] [--cancel-after S] [--keep]
#                    [--harness METRICS_JSON] [--toml SECTION.KEY=VALUE ...] [--name N]
#                    [--cleanup] [--existing] [--cleanup-opts all|user|pcbnew-json|BOX,BOX]
# Needs: Xephyr, metacity, xdotool, wmctrl, xwd, ImageMagick convert, dbus-run-session, KiCad 10, network on the
# first run (KiCad's pip installs kicad-python into the test plugin environment). Steps and checks:
# harness/gui_e2e.py; docs/user/gui-test-checklist.md#automated-run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV="${TOPOR_VENV:-$ROOT/.venv}"
export PYTHONPATH="$ROOT:$ROOT/plugin${PYTHONPATH:+:$PYTHONPATH}"
exec "$VENV/bin/python" -P "$ROOT/harness/gui_e2e.py" "$@"
