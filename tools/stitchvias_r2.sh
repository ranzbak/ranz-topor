#!/usr/bin/env bash
# stitch-vias r2 (local, one slot): det s0 tip runs, each measured after KiCad's own refill
# (tools/stitchvias_measure.py) and drawn with the refilled pours (tools/stitchvias_png.py).
#   wiccon arms for D1: p5 (defaults), p7.5, p10, noarea (stitch_per_area = false, pitch 5)
#   regression of F1 / D2: eprom, esp, qmtech at the defaults; the PROPOSED pattern
#   usage: TOPOR_VENV=<venv> tools/stitchvias_r2.sh OUT [arm ...]     (arms: p5 p7.5 p10 noarea stag ep esp qm pattern; wicoff epoff espoff qmoff = stitching off)
set -u
OUT=$(realpath -m "$1"); shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
PY="${TOPOR_VENV:-$ROOT/.venv}/bin/python"
# stitching as before stitch-vias (one via per cut piece, no region classes): the same-rev baseline (r3)
OFF=(--set planes.stitch_dropped=false --set planes.stitch_thin_mm=0 --set planes.stitch_min_vias=1 --set planes.stitch_per_area=false)
S=(--set optimize.runs=1 --set optimize.rounds=1 --set optimize.stable_patience=1)
B=$ROOT/../../boards
[ -d "$B" ] || B=$ROOT/scratch/boards
WIC=$B/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
EP=$(ls "$B"/eprom-to-kernal/*/eprom-to-kernal.kicad_pcb | head -1)
ESP=$B/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
QM=$B/qmtech-hand/qmtech-minimig.kicad_pcb
run() { # name board [--set ...]
  local name=$1 pcb=$2; shift 2
  local stem; stem=$(basename "$pcb" .kicad_pcb)
  "$PY" -m harness.score_runs "$OUT/$name" "$pcb" --only 0 --no-png --threads 4 "${S[@]}" "$@" > "$OUT/$name.log" 2>&1
  local routed=$OUT/$name/$stem/s0/$stem-topor/$stem.kicad_pcb
  "$PY" -m tools.stitchvias_measure "$routed" --out "$OUT/$name-measure.json" --work "$OUT/$name-measure-work" >> "$OUT/$name.log" 2>&1
  local res; res=$(ls "$OUT/$name/$stem/s0/reports"/*/*/result.json 2>/dev/null | sort | tail -1)
  "$PY" -m tools.stitchvias_png --out "$OUT/$name.png" \
    --panel "$name: $stem det s0, refilled by KiCad (F.Cu red, B.Cu blue)" "$OUT/$name-measure-work/$stem.kicad_pcb" "${res:--}" >> "$OUT/$name.log" 2>&1
  echo "$name done rc $?" >> "$OUT/rc.txt"
}
pattern() {
  "$PY" -m harness.stitch_patterns "$OUT/pattern-boards" > "$OUT/pattern.log" 2>&1
  local P=$OUT/pattern-boards/stitch_pieces/problem/stitch_pieces.kicad_pcb
  TOPOR_HAND_LABEL=problem TOPOR_VENV="${TOPOR_VENV:-}" tools/route_board.sh "$P" "$OUT/pattern" --keep-copper --no-png >> "$OUT/pattern.log" 2>&1
  local routed=$OUT/pattern/stitch_pieces-topor/stitch_pieces.kicad_pcb
  "$PY" -m tools.stitchvias_measure "$routed" --out "$OUT/pattern-measure.json" --work "$OUT/pattern-measure-work" >> "$OUT/pattern.log" 2>&1
  local res; res=$(ls "$OUT/pattern/reports"/*/*/result.json 2>/dev/null | sort | tail -1)
  "$PY" -m tools.stitchvias_png --out "$OUT/pattern.png" \
    --panel "PROPOSED (awaiting approval): stitch_pieces, refilled by KiCad (F.Cu red, B.Cu blue)" \
    "$OUT/pattern-measure-work/stitch_pieces.kicad_pcb" "${res:--}" "$OUT/pattern-boards/stitch_pieces/expected.json" >> "$OUT/pattern.log" 2>&1
  echo "pattern done rc $?" >> "$OUT/rc.txt"
}
mkdir -p "$OUT"
for a in "${@:-p5 p7.5 p10 noarea ep esp qm pattern}"; do
  for arm in $a; do
    case $arm in
      p5) run p5 "$WIC" ;;
      p7.5) run p7.5 "$WIC" --set planes.stitch_pitch_mm=7.5 ;;
      p10) run p10 "$WIC" --set planes.stitch_pitch_mm=10 ;;
      noarea) run noarea "$WIC" --set planes.stitch_per_area=false ;;
      ep) run ep "$EP" ;;
      esp) run esp "$ESP" ;;
      qm) run qm "$QM" ;;
      stag) run stag "$WIC" --set planes.stitch_lattice=staggered ;;
      wicoff) run wicoff "$WIC" "${OFF[@]}" ;;
      epoff) run epoff "$EP" "${OFF[@]}" ;;
      espoff) run espoff "$ESP" "${OFF[@]}" ;;
      qmoff) run qmoff "$QM" "${OFF[@]}" ;;
      pattern) pattern ;;
    esac
  done
done
echo "all done" >> "$OUT/rc.txt"
