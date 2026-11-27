#!/usr/bin/env bash
# bundle-turn fix round r2 (one ccx43 hour): RCA + rule 14c + mutants.
#   bundle_turn_r2fix_job.sh top    the hzq units below, then "all done"
#   ... rca                         qm + esp det s0 C1, arms tip (defaults) / obl0 (capacity.oblique_price=0), TOPO_LOOPDBG
#   main (316e86ef): its own `hz.sh qsub --rev 316e86ef` unit, the same score_runs line inline (main has no copy of this script)
#   ... prof                        qm tip det s0 C1 under perf (tools/prof_run.sh --flame, else perf report)
#   ... suite                       rule 14c: every test binary once (release, --no-fail-fast)
#   ... mut                         item-2 / item-4 mutants against the capacity / drawing test binaries
#   ... crops                       after rca + main: three-panel crops of the item-4 nets (tools/bt_r2_crops.py)
set -u
OUT=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $OUT/rc.txt; }
git rev-parse HEAD > $OUT/rev.txt 2>/dev/null || cat REV > $OUT/rev.txt 2>/dev/null
C1="--set optimize.runs=1 --set optimize.rounds=1 --set capacity.compass_cut_check=true"
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() {  # unit ARM BOARD PCB THREADS [sets]: score_runs det s0 C1 with LOOPDBG (stderr into the log, gzipped after)
  local a=$1 n=$2 p=$3 th=$4; shift 4
  TOPO_LOOPDBG=1 TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads $th $C1 "$@" > "$OUT/$a-$n.log" 2>&1 9>&-
  note "$a-$n rc $?"; gzip -f "$OUT/$a-$n.log"
}
case "${1:-}" in
  rca)
    unit tip qm $QM 3 & unit obl0 qm $QM 3 --set capacity.oblique_price=0 &
    unit tip esp $ESP 1 & unit obl0 esp $ESP 1 --set capacity.oblique_price=0 & wait ;;
  prof)
    mkdir -p $OUT/prof
    if command -v inferno-flamegraph >/dev/null; then
      RAYON_NUM_THREADS=4 tools/prof_run.sh $QM $OUT/prof --flame $C1 > $OUT/prof.log 2>&1 9>&-
    else
      TOPO_PROF=1 RAYON_NUM_THREADS=4 perf record -F 299 -g -o $OUT/prof/perf.data -- tools/route_board.sh $QM $OUT/prof/run --clock cpu --timeout 2400 $C1 --no-png > $OUT/prof.log 2>&1 9>&-
      perf report -i $OUT/prof/perf.data --no-children --sort symbol --stdio 2>/dev/null | head -150 > $OUT/prof/report-self.txt
      perf report -i $OUT/prof/perf.data --children --sort symbol --stdio 2>/dev/null | grep -E "^ +[0-9]" | head -120 > $OUT/prof/report-children.txt
      rm -f $OUT/prof/perf.data
    fi
    note "prof rc $?" ;;
  suite)
    ( cd engine && CARGO_TARGET_DIR=/root/target-bt2s cargo test --release --no-fail-fast > $OUT/suite.log 2>&1 9>&- ); note "suite rc $?"
    grep -E "^test .* FAILED|^error: test failed|test result: FAILED" $OUT/suite.log > $OUT/suite-fails.txt; note "suite fails $(grep -c 'FAILED' $OUT/suite-fails.txt)" ;;
  mut)
    BINS="--test topo_bundle_turn --test topo_first_draw_45 --test topo_thick_layer --test topo_dvn --test topo_m3_patterns --test topo_drawing_grace --test topo_site_social --test tw5_escape --test loop_local --test topo_cm1_room --test topo_q14 --test topo_p5_price --test topo_disc_height"
    M=/root/bt2mut; rm -rf $M; mkdir -p $M
    git ls-files engine Cargo.toml Cargo.lock | tar -cf - -T - | (cd $M && tar -xf -)
    mut() {  # mut NAME FILE PYTHON-OLD PYTHON-NEW [lib]
      local n=$1 f=$2 old=$3 new=$4 lib=${5:-}
      ( cd $M && git -C $ROOT show HEAD:$f > $f ) 2>/dev/null || cp $ROOT/$f $M/$f
      python3 - "$M/$f" "$old" "$new" <<'EOF' || { note "mut $n: pattern not found"; return; }
import sys
p, old, new = sys.argv[1:4]
s = open(p).read()
assert s.count(old) == 1, s.count(old)
open(p, "w").write(s.replace(old, new))
EOF
      ( cd $M/engine && CARGO_TARGET_DIR=/root/target-bt2m cargo test --release --no-fail-fast ${lib:+--lib} $BINS > $OUT/mut-$n.log 2>&1 9>&- ); local rc=$?
      note "mut $n rc $rc (killed if != 0): $(grep -E '^test .* FAILED' $OUT/mut-$n.log | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')"
      cp $ROOT/$f $M/$f
    }
    mut fits_without_old engine/src/topo/refine.rs "b.fits() || self.fx.packer.oblique_split(e, &items, &isq, &b.lo, &b.hi, b.worst(), &mut Vec::new(), &mut Vec::new(), &mut Vec::new()).0 <= 1e-6" "b.fits()"
    mut eval_over_kept engine/src/topo/refine.rs "let (over, obl) = self.packer.oblique_split(e, items, &sc.isq, &sc.lo, &sc.hi, over, &mut sc.square, &mut sc.req, &mut sc.rmax);" "let (over, obl) = (over, self.packer.oblique_split(e, items, &sc.isq, &sc.lo, &sc.hi, over, &mut sc.square, &mut sc.req, &mut sc.rmax).1);"
    mut lane_pack_off engine/src/topo/draw.rs "if !bnd.fits() && self.packer.oblique_price > 0.0 && pitems.iter().any(|i| i.infl > 1.0) {" "if false && !bnd.fits() && self.packer.oblique_price > 0.0 && pitems.iter().any(|i| i.infl > 1.0) {"
    mut reach_one engine/src/topo/pack.rs "let ang = |v: Vec2| v.y.atan2(v.x);" "if true { return 1.0; } let ang = |v: Vec2| v.y.atan2(v.x);" lib
    mut local_no_grow engine/src/topo/exact.rs "                Err(hits) => {
                    let n0 = set.len();" "                Err(_) if true => return None,
                Err(hits) => {
                    let n0 = set.len();" lib
    mut relax_all engine/src/topo/exact.rs "let relax = if wp.relax_on.is_none_or(|r| r.contains(id)) { wp.relax } else { 0.0 };" "let relax = wp.relax;" lib
    # not a mutant: the three loop tests' numbers at oblique price 0 (main's capacity model), for their verdicts
    sed -i 's/^            oblique_price: 1\.0,$/            oblique_price: 0.0,/' $M/engine/src/model.rs
    ( cd $M/engine && CARGO_TARGET_DIR=/root/target-bt2m cargo test --release --no-fail-fast --test loop_local --test tw5_escape --test topo_site_social -- --nocapture local_depth_two loop_p4_rounds_fold a_strict_detour > $OUT/obl0-loops.log 2>&1 9>&- ); note "obl0 loop tests rc $? (default changed: $(grep -c 'oblique_price: 0.0,' $M/engine/src/model.rs))"
    cp $ROOT/engine/src/model.rs $M/engine/src/model.rs
    ( cd engine && TOPO_FD45_OFF=relaxloc CARGO_TARGET_DIR=/root/target-bt2s cargo test --release --no-fail-fast $BINS > $OUT/mut-env-relaxloc.log 2>&1 9>&- )
    note "mut env-relaxloc rc $? (killed if != 0): $(grep -E '^test .* FAILED' $OUT/mut-env-relaxloc.log | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')" ;;
  crops)
    NETS_QM="JS_CS JS_MOSI ADV_D6 ADV_CEC FPGA_SD_CLK USB_VBUS"; NETS_ESP="IO4 IO8_SCL IO10_SDA GND"
    pcb() { echo $OUT/$1-$2/$3/s0/$3-topor/$3.kicad_pcb; }
    python tools/bt_r2_crops.py $OUT/crops/qm $(pcb main qm qmtech-minimig) $(pcb obl0 qm qmtech-minimig) $(pcb tip qm qmtech-minimig) $NETS_QM > $OUT/crops-qm.log 2>&1; note "crops qm rc $?"
    python tools/bt_r2_crops.py $OUT/crops/esp $(pcb main esp esp-rust-board) $(pcb obl0 esp esp-rust-board) $(pcb tip esp esp-rust-board) $NETS_ESP > $OUT/crops-esp.log 2>&1; note "crops esp rc $?" ;;
  top)
    T=${HZ_TAG:-bt2f}
    hzq run --threads 8 --name $T-rca -- bash $ROOT/tools/bundle_turn_r2fix_job.sh rca 9>&- & sleep 3
    hzq run --threads 4 --name $T-prof -- bash $ROOT/tools/bundle_turn_r2fix_job.sh prof 9>&- & sleep 3
    hzq run --threads 8 --name $T-suite -- bash $ROOT/tools/bundle_turn_r2fix_job.sh suite 9>&- & sleep 3
    hzq run --threads 6 --name $T-mut -- bash $ROOT/tools/bundle_turn_r2fix_job.sh mut 9>&- &
    wait
    note "all done" ;;
  *) echo "usage: $0 top|rca|main|prof|suite|mut|crops" >&2; exit 2 ;;
esac
