#!/usr/bin/env bash
# Freeze the classic reference (S6 gate): build main @ 64b3979 in a temp worktree, run it once per board with
# tools/route_board.sh (same metric code as for topor-topo), write the table, remove the worktree.
#   tools/classic_reference.sh <out_dir> [--timeout S] [board ...]
# Boards (default: all): qmtech qmtech-usb-crop dcsgone-usb-crop eprom-to-kernal wiccon-2023 tanmatsu bornhack2026 hh2027
# Environment: TOPOR_BOARDS (default <repo>/scratch/boards; in a git worktree point it at the main tree's),
#   TOPOR_VENV, CLASSIC_COMMIT (default 64b3979), CLASSIC_KEEP=1 keeps the temp worktree.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VENV="${TOPOR_VENV:-$ROOT/.venv}"
B="${TOPOR_BOARDS:-$ROOT/scratch/boards}"
COMMIT="${CLASSIC_COMMIT:-64b3979}"
OUT="$(mkdir -p "$1" && cd "$1" && pwd)"; shift
TIMEOUT=600
if [ "${1:-}" = "--timeout" ]; then TIMEOUT="$2"; shift 2; fi
WANT=("$@"); [ ${#WANT[@]} -eq 0 ] && WANT=(qmtech qmtech-usb-crop dcsgone-usb-crop eprom-to-kernal wiccon-2023 tanmatsu bornhack2026 hh2027)
want() { for w in "${WANT[@]}"; do [ "$w" = "$1" ] && return 0; done; return 1; }

TMP="$(mktemp -d)"
WT="$TMP/classic"
cleanup() { [ "${CLASSIC_KEEP:-0}" = 1 ] || { git -C "$ROOT" worktree remove --force "$WT" 2>/dev/null || true; rm -rf "$TMP"; }; }
trap cleanup EXIT
git -C "$ROOT" worktree add --detach "$WT" "$COMMIT" >/dev/null
"$VENV/bin/maturin" build --release --manifest-path "$WT/engine/Cargo.toml" --features python --out "$TMP/wheel" >"$OUT/classic-build.log" 2>&1
"$VENV/bin/python" -m pip install -q --no-deps --target "$TMP/site" "$TMP"/wheel/topor_engine-*.whl
export TOPOR_TREE="$WT" TOPOR_ENGINE_SITE="$TMP/site"
git -C "$WT" log --oneline -1 | tee "$OUT/classic-commit.txt"

run_board() { # name pcb [extra args]
    local name="$1" pcb="$2"; shift 2
    want "$name" || return 0
    echo "== $name"; "$ROOT/tools/route_board.sh" "$pcb" "$OUT/$name" --name "$name" --timeout "$TIMEOUT" "$@" 2>&1 \
        | grep -v "Debug: Adding duplicate" | tail -12
}
run_crop() { # name fixture
    want "$1" || return 0
    echo "== $1"; git -C "$WT" show "$COMMIT:engine/tests/fixtures/$2" >"$OUT/$2"
    PYTHONPATH="$TOPOR_ENGINE_SITE:$WT" "$VENV/bin/python" -P "$ROOT/harness/classic_ref.py" crop "$OUT/$2" "$1" "$OUT/crop-$1.json"
}
run_board qmtech "$B/qmtech-rec-on/qmtech-minimig.kicad_pcb"
run_crop qmtech-usb-crop qmtech-usb-crop.json
run_crop dcsgone-usb-crop dcsgone-usb-crop.json
run_board eprom-to-kernal "$B/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb"
run_board wiccon-2023 "$B/wiccon-2023/2d3063e70ed5/2023.kicad_pcb"
run_board tanmatsu "$B/tanmatsu-copy/tanmatsu.kicad_pcb"
run_board bornhack2026 "$B/bornhack2026/21c5f478f1af/bornhack2026-hardware.kicad_pcb"
run_board hh2027 "$B/hh2027-hand/hackerhotel2027.kicad_pcb" --whole-project
PYTHONPATH="$ROOT" "$VENV/bin/python" -P "$ROOT/harness/classic_ref.py" table "$OUT" "$OUT/classic-reference"
