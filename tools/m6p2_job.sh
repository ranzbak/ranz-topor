#!/usr/bin/env bash
# m6p2_job.sh: 6b/M6 phase 2 (branch m6-fire): copper before/after the R1-R5 removal, the hard-refusal set
# measured (H1 = [capacity] edges_test = "refuse"), targeted tests + mutants, the bornhack EPD_MOSI repro.
#   m6p2_job.sh main base     (rev = main + this script + the untangle problem boards) default config only
#   m6p2_job.sh main tip      (rev = m6-fire + the same) D and H1, tests + mutants, repro
#   m6p2_job.sh unit T CFG... / task CFG KIND NAME / tests / repro   (inside a unit)
# Runs: seed 0, optimize.rounds=1 runs=1, 1 thread each; public boards eprom, wiccon, esp, qmtech via
# harness.score_runs; patterns fit (7) + via (6 approved) + radii (4 approved) from their generators, untangle (5)
# from m6-job/boards (the 5 fanout boards have 0 connections at b90ec1a, phase 1 F-4: left out).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(cd "$(dirname "$0")/.." && pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
BOARDS="qmtech esp wiccon eprom"
VIA="via_pair_redundant via_pair_slide via_force_relocate branch_point_relocate branch_point_into_pad via_size_rule"
cfg() {
  SETS=()
  case $1 in
    D) ;;
    H1) SETS=(--set capacity.edges_test=refuse) ;;
    *) echo "m6p2_job: unknown config $1" >&2; exit 2 ;;
  esac
}
board_path() { case $1 in eprom) echo eprom-to-kernal;; wiccon) echo wiccon-2023;; esp) echo $E;; qmtech) echo $Q;; esac; }
patterns() { ls -d $R/pat/*/ | xargs -n1 basename; }
task() {
  local c=$1 k=$2 n=$3; cfg $c
  export RAYON_NUM_THREADS=1
  mkdir -p $R/$c/$k
  if [ $k = b ]; then
    python -m harness.score_runs $R/$c/b/$n "$(board_path $n)" --only 0 --no-png --threads 1 \
      --set optimize.rounds=1 --set optimize.runs=1 "${SETS[@]}" > $R/$c/b-$n.log 2>&1
  else
    PYTHONPATH=$ROOT:$ROOT/plugin python -P harness/route_board.py $R/pat/$n/problem/$n.kicad_pcb $R/$c/p/$n --no-png \
      --set optimize.rounds=1 --set optimize.runs=1 "${SETS[@]}" > $R/$c/p-$n.log 2>&1
  fi
  note "task $c $k $n rc $? sets=[${SETS[*]}]"
}
tests() {
  cd $ROOT/engine
  local T="cargo test --release --no-fail-fast"
  { $T --test topo_m6 --test topo_mb1 --test topo_mc --test topo_vias54 --test topo_d6b --test topo_mb0 2>&1; echo "itests rc $?"
    $T --test topo_m2_draw_once regression_guard 2>&1; echo "m2 guard rc $?"
    $T --lib -- topo::draw topo::octi topo::core 2>&1; echo "lib rc $?"
    cargo clippy --release --all-targets -- -D warnings 2>&1 | tail -30; echo "clippy rc ${PIPESTATUS[0]}"
  } > $R/tests.log
  cd $ROOT; CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_messages.py > $R/pytest.log 2>&1
  note "pytest rc $?"; grep -E "test result|rc " $R/tests.log | while read -r l; do note "tests: $l"; done
  mutants
}
mut() {  # name file test-args... ; the python snippet on stdin edits the file
  local n=$1 f=$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  ( cd $ROOT/engine && cargo test --release "$@" > $R/mut-$n.log 2>&1 ); local rc=$?
  mv $f.orig $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
sub() { cat <<EOF
import sys; p = sys.argv[1]; s = open(p).read(); a = '''$1'''; b = '''$2'''
assert s.count(a) == 1, a; open(p, 'w').write(s.replace(a, b))
EOF
}
mutants() {
  local C=$ROOT/engine/src/topo/core.rs D=$ROOT/engine/src/topo/draw.rs
  sub 'let feedback = opts.drawing_feedback && opts.opt_loop == "cycles";' 'let feedback = opts.drawing_feedback;' | mut m1_feedback_gate $C --test topo_m6
  sub 'warnings.push(FEEDBACK_IGNORED.to_string());' '' | mut m2_feedback_warning $C --test topo_m6
  sub '    if last.is_empty() {
        no_run_reason' '    if false {
        no_run_reason' | mut m3_blank_reason $D --lib -- topo::draw::tests::a_run_that_drew_nothing
  sub 'if angle_gain >= 0.0 { "lower" } else { "higher" }' 'if angle_gain >= 0.0 { "higher" } else { "lower" }' | mut m4_fuse_angle_word $C --test topo_m6
  sub 'out.push((gain, along - d, i, pts));' 'out.push((gain, gain, i, pts));' | mut m5_fuse_len_part $D --lib -- topo::draw::tests::fuse
  sub 'let cycles = opts.opt_loop == "cycles";' 'let cycles = false;' | mut m6_cycles_memory_off $C --test topo_mb0
  sub 'let angle_gain: f64 = fused.iter().map(|f| f.0 - f.1).sum();' 'let angle_gain: f64 = fused.iter().map(|f| f.0).sum();' | mut m7_fuse_angle_sum $C --test topo_m6
}
tests2() {  # round 2: the cycles memory line + fuse_line over the applied fuses; the two earlier survivors again
  cd $ROOT/engine
  { cargo test --release --no-fail-fast --test topo_m6 --test topo_mb0 2>&1; echo "itests2 rc $?"; } > $R/tests2.log
  grep -E "test result|rc " $R/tests2.log | while read -r l; do note "tests2: $l"; done
  cd $ROOT; local C=$ROOT/engine/src/topo/core.rs
  sub 'let cycles = opts.opt_loop == "cycles";' 'let cycles = false;' | mut m6_cycles_memory_off $C --test topo_m6
  sub 'if cycles {
        warnings.push(format!(' 'if !cycles {
        warnings.push(format!(' | mut m6b_cycles_line_gate $C --test topo_m6
  sub 'let angle_gain: f64 = fused.iter().map(|f| f.0 - f.1).sum();' 'let angle_gain: f64 = fused.iter().map(|f| f.0).sum();' | mut m7_fuse_angle_sum $C --test topo_m6
  sub 'let len_gain_mm: f64 = fused.iter().map(|f| f.1).sum();' 'let len_gain_mm: f64 = fused.iter().map(|f| f.0).sum();' | mut m8_fuse_len_sum $C --test topo_m6
  note "tests2 done"
}
tests3() {  # fix round 3 (verify 2026-11-18): V-2 fuse line on a routed board, F-1 as a unit test, V-5 line
  cd $ROOT/engine
  { cargo test --release --no-fail-fast --test topo_m6 --test topo_mb0 --test topo_mb1 --test topo_mc --test topo_d6b --test topo_vias54 9>&- 2>&1; echo "itests3 rc $?"
    cargo test --release --test topo_m2_draw_once regression_guard 9>&- 2>&1; echo "m2 guard rc $?"
    cargo test --release --lib -- topo::draw topo::octi topo::core 9>&- 2>&1; echo "lib rc $?"; } > $R/tests3.log
  grep -E "test result|rc " $R/tests3.log | while read -r l; do note "tests3: $l"; done
  cd $ROOT; CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_messages.py > $R/pytest3.log 2>&1
  note "pytest3 rc $?"
  local C=$ROOT/engine/src/topo/core.rs D=$ROOT/engine/src/topo/draw.rs P=$ROOT/plugin/topor/config.py
  sub 'fused.push((gain, len_gain));' 'fused.push((gain, gain));' | mut v9_fuse_wiring $C --test topo_m6 the_fuse_line_of_a_routed_board
  sub '    if last.is_empty() {
        no_run_reason' '    if false {
        no_run_reason' | mut f1_failed_run_reason $D --lib -- topo::draw::tests::a_run_that_drew_nothing
  sub '{CONFIG_NAME}{_line_text(line)} is ignored:' '{CONFIG_NAME} is ignored:' | pymut v5_removed_line $P tests/plugin/test_config.py -k m6_removed
  sub 'findings.append(_removed(sec, key, key_line(text, sec, key)))
            continue
        if sec not in SECTIONS' 'findings.append(_removed(sec, key))
            continue
        if sec not in SECTIONS' | pymut v5b_removed_section_line $P tests/plugin/test_config.py -k m6_removed
  note "tests3 done"
}
tests3py() {  # fix round 3b: the plugin half again (3a's pytest file did not parse, so its python mutants proved nothing)
  cd $ROOT; CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_messages.py > $R/pytest3.log 2>&1
  local rc=$?; note "pytest3 rc $rc"; tail -1 $R/pytest3.log | while read -r l; do note "pytest3: $l"; done
  [ $rc -eq 0 ] || { note "tests3py: pytest red, mutants skipped"; return; }
  local P=$ROOT/plugin/topor/config.py
  sub '{CONFIG_NAME}{_line_text(line)} is ignored:' '{CONFIG_NAME} is ignored:' | pymut v5_removed_line $P tests/plugin/test_config.py -k m6_removed
  sub 'findings.append(_removed(sec, key, key_line(text, sec, key)))
            continue
        if sec not in SECTIONS' 'findings.append(_removed(sec, key))
            continue
        if sec not in SECTIONS' | pymut v5b_removed_section_line $P tests/plugin/test_config.py -k m6_removed
  note "tests3py done"
}
pymut() {  # name file pytest-args... ; like mut, for the plugin
  local n=$1 f=$2; shift 2
  cp $f $f.orig; python3 - $f; local pr=$?
  if [ $pr -ne 0 ] || cmp -s $f $f.orig; then note "mutant $n: patch did not apply"; mv $f.orig $f; return; fi
  ( cd $ROOT && CI=1 python -m pytest -q "$@" > $R/mut-$n.log 2>&1 ); local rc=$?
  mv $f.orig $f
  if [ $rc -ne 0 ]; then note "mutant $n: killed"; else note "mutant $n: SURVIVED"; fi
}
repro() {  # bornhack EPD_MOSI: KiCad sees U6's footprint clearance 0.127 mm, does the exported model?
  local out=$R/repro; mkdir -p $out
  python -m harness.g0 export bornhack2026 $out/model > $out/export.log 2>&1; note "repro export rc $?"
  python - $out bornhack2026 > $out/repro.txt 2>&1 <<'EOF'
import glob, json, sys
out = sys.argv[1]
ms = sorted(glob.glob(out + "/model/**/*.json", recursive=True))
print("models:", ms)
for m in ms:
    d = json.load(open(m))
    if not isinstance(d, dict) or "pads" not in d:
        continue
    u6 = [p for p in d["pads"] if p.get("ref") == "U6"]
    lc = sorted({p.get("local_clearance") for p in u6}, key=str)
    p74 = [p for p in u6 if str(p.get("number")) == "74"][:1]
    print(m, "U6 pads", len(u6), "local_clearance values", lc, "pad 74", p74[0].get("local_clearance") if p74 else None,
          "net rules clearance", (d.get("rules") or {}).get("clearance"))
import harness  # noqa: F401  (isolated KiCad home before pcbnew; cwd = repo root)
import pcbnew
pcbs = sorted(glob.glob(out + "/model/*-work/*.kicad_pcb"))
print("work copy:", pcbs)
b = pcbnew.LoadBoard(pcbs[0])
fp = b.FindFootprintByReference("U6")
print("footprint U6 GetLocalClearance:", fp.GetLocalClearance())
for pad in fp.Pads():
    if pad.GetNumber() == "74":
        print("pad 74 at", pad.GetPosition(), "GetLocalClearance:", pad.GetLocalClearance(),
              "GetOwnClearance(F.Cu):", pad.GetOwnClearance(pcbnew.F_Cu))
        break
EOF
  note "repro rc $?"
}
gen() {
  mkdir -p $R/pat
  PYTHONPATH=$ROOT:$ROOT/plugin python - "$R/pat" $VIA > $R/gen.log 2>&1 <<'EOF'
import sys
from pathlib import Path
from harness import fit_patterns, via_patterns, radii_patterns
out = Path(sys.argv[1])
for n in fit_patterns.PATTERNS:
    fit_patterns.generate(n, out, png=False)
for n in sys.argv[2:]:
    via_patterns.generate(n, out, png=False, drc=False)
for n in radii_patterns.PATTERNS:
    radii_patterns.generate(n, out, png=False, drc=False)
EOF
  note "gen rc $?"
  for d in m6-job/boards/*/; do case $(basename $d) in fanout_*) ;; *) cp -r $d $R/pat/ ;; esac; done
  note "patterns: $(patterns | tr '\n' ' ')"
}
case ${1:-} in
  task) shift; task "$@" ;;
  tests) tests ;;
  tests2) tests2 ;;
  tests3) tests3 ;;
  tests3py) tests3py ;;
  repro) repro ;;
  unit) shift; T=$1; shift
    { for n in $BOARDS; do for c in "$@"; do echo "$c b $n"; done; done
      for n in $(patterns); do for c in "$@"; do echo "$c p $n"; done; done; } | xargs -P $T -L1 bash $ROOT/tools/m6p2_job.sh task ;;
  main)
    gen
    u() { local n=$1 t=$2; shift 2; ( hzq run --threads $t --name "$n" -- bash $ROOT/tools/m6p2_job.sh "$@" > $R/unit-$n.log 2>&1; note "unit $n rc $?" ) & sleep 2; }
    case ${2:-} in
      base) u m6b-d 8 unit 8 D ;;
      tip) u m6t-tests 4 tests; u m6t-d 8 unit 8 D; u m6t-h1 8 unit 8 H1; u m6t-repro 1 repro ;;
      r3) u m6r3-tests 4 tests3; u m6r3-esp 1 task D b esp ;;
      *) echo "main base|tip|r3" >&2; exit 2 ;;
    esac
    wait; note "all done" ;;
  *) sed -n 2,9p "$0"; exit 2 ;;
esac
