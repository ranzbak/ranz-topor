#!/usr/bin/env bash
# parocti_job.sh (branch par-octi = par-draw A + octilinear lanes on main 01e85fce, Hetzner): acceptance.
#   all : par tests (lib par:: octi exact::, tests/par_draw at --test-threads 1), near tests, config pytest, clippy;
#         board copper hashes (esp/qmtech x sequential/erbs x RAYON 2/4, eprom/wiccon x sequential/erbs x RAYON 1,
#         rounds=1 runs=1); then the mutants M1-M4 (tools/parocti_mut.py).
#   ref : the board hashes of this rev at RAYON 4 only (run it on main for "copper = main").
#   time (timebase: a rev without draw_lanes): qmtech + esp, sequential/erbs: runs=1 at RAYON 1 and 8, runs=4 at RAYON 8 with draw_lanes auto and 1 (exclusive VM).
# Runs in the repo root (ROOT = pwd), so the same text works for a rev without this file (base64 in the command).
set -u
R=${HZ_RUN_DIR:?}; ROOT=$(pwd); cd $ROOT
note() { echo "$*" >> $R/rc.txt; }
A="--clock cpu --timeout 2400 --no-png"
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
W=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
P=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb
# one NAME PCB THREADS [route_board args]: route, then hash the copper
one() { local n=$1 b=$2 t=$3; shift 3
  local o=$R/o/$n; mkdir -p $o
  hzq run --threads $t --name pd-$n -- env RAYON_NUM_THREADS=$t TOPO_TIMES=1 tools/route_board.sh $b $o $A "$@" > $R/$n.log 2>&1 9>&-
  local rc=$? pcb; pcb=$(ls $o/*-topor/*.kicad_pcb 2>/dev/null | head -1)
  echo "$n rc $rc $(python3 tools/fx_hash.py $pcb $o/none 2>/dev/null || echo nohash)" >> $R/hash.txt; }
boards() { # boards THREADS...
  for t in "$@"; do
    for bb in eprom:$P wiccon:$W esp:$E qmtech:$Q; do
      n=${bb%%:*}; b=${bb#*:}
      one $n-seq-t$t $b $t --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=sequential & sleep 1
      one $n-erbs-t$t $b $t --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=erbs & sleep 1
    done
  done; }
case ${1:-all} in
  all)
    ( hzq run --threads 4 --name pd-tests -- bash -c "cd $ROOT/engine && RAYON_NUM_THREADS=4 cargo test --release --lib -- par:: octi exact:: 9>&- > $R/tests.log 2>&1; echo \"lib rc \$?\" >> $R/tests.log; RAYON_NUM_THREADS=4 cargo test --release --test par_draw -- --test-threads 1 9>&- >> $R/tests.log 2>&1; echo \"par_draw rc \$?\" >> $R/tests.log; RAYON_NUM_THREADS=4 cargo test --release --test par_refine --test par_refine_dense --test topo_drawing_grace --test topo_m3_class_keep 9>&- >> $R/tests.log 2>&1; echo \"near rc \$?\" >> $R/tests.log; cargo clippy --release --all-targets 9>&- >> $R/tests.log 2>&1; echo \"clippy rc \$?\" >> $R/tests.log; cd $ROOT && CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_toml_defaults.py tests/test_docs_links.py >> $R/tests.log 2>&1; echo \"pytest rc \$?\" >> $R/tests.log" ) & sleep 1
    if [ -z "${NOBOARDS:-}" ]; then  # NOBOARDS=1: tests and mutants only
    # slim (lead 2026-11-18, smart compute): esp/qmtech at RAYON 2 and 4 (1 and 8 come from the
    # exclusive timing run), eprom/wiccon at RAYON 1; no default-runs configs
    for t in 2 4; do
      for bb in esp:$E qmtech:$Q; do
        n=${bb%%:*}; b=${bb#*:}
        one $n-seq-t$t $b $t --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=sequential & sleep 1
        one $n-erbs-t$t $b $t --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=erbs & sleep 1
      done
    done
    for bb in eprom:$P wiccon:$W; do
      n=${bb%%:*}; b=${bb#*:}
      one $n-seq-t1 $b 1 --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=sequential & sleep 1
      one $n-erbs-t1 $b 1 --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=erbs & sleep 1
    done
    fi
    wait
    grep -E "rc [0-9]|^test result|FAILED|panicked|^(warning|error)(\[|:)" $R/tests.log | while read -r l; do note "tests: $l"; done
    # mutants (tools/parocti_mut.py), each from the clean sources
    cd $ROOT/engine; cp src/topo/par.rs $R/par.rs.orig; cp src/topo/octi.rs $R/octi.rs.orig
    for M in M1 M2 M3 M4; do
      cp $R/par.rs.orig src/topo/par.rs; cp $R/octi.rs.orig src/topo/octi.rs
      python3 $ROOT/tools/parocti_mut.py $M; note "mutant $M applied rc $?"
      hzq run --threads 4 --name pd-mut$M -- bash -c "RAYON_NUM_THREADS=4 cargo test --release --lib -- octilinear_shape_cache par:: 9>&- > $R/mut$M.log 2>&1; echo \"lib rc \$?\" >> $R/mut$M.log; RAYON_NUM_THREADS=4 cargo test --release --test par_draw -- --test-threads 1 9>&- >> $R/mut$M.log 2>&1; echo \"par_draw rc \$?\" >> $R/mut$M.log"
      grep -E "rc [0-9]|^test result" $R/mut$M.log | while read -r l; do note "mutant $M: $l"; done
    done
    cp $R/par.rs.orig src/topo/par.rs; cp $R/octi.rs.orig src/topo/octi.rs
    note "all done" ;;
  ref)
    for bb in eprom:$P wiccon:$W esp:$E qmtech:$Q; do
      n=${bb%%:*}; b=${bb#*:}
      one $n-seq-t4 $b 4 --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=sequential & sleep 1
      one $n-erbs-t4 $b 4 --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=erbs & sleep 1
    done
    one eprom-def-t4 $P 4 --set drawing.realise=erbs & sleep 1
    one wiccon-def-t4 $W 4 --set drawing.realise=erbs & sleep 1
    wait; note "all done" ;;
  prof)  # the TOPO_TIMES par-draw meters: qmtech + esp erbs/sequential, runs=1, RAYON 1 and 8
    for bb in qmtech:$Q esp:$E; do
      n=${bb%%:*}; b=${bb#*:}
      for r in sequential erbs; do
        for t in 1 8; do one $n-$r-r1-t$t $b $t --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=$r; done
      done
    done
    note "all done" ;;
  time|timebase)
    for bb in qmtech:$Q esp:$E; do
      n=${bb%%:*}; b=${bb#*:}
      for r in sequential erbs; do
        one $n-$r-r1-t1 $b 1 --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=$r
        one $n-$r-r1-t8 $b 8 --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=$r
        one $n-$r-r4-t8 $b 8 --set optimize.rounds=1 --set optimize.runs=4 --set drawing.realise=$r
        [ $1 = time ] && one $n-$r-r4-t8-l1 $b 8 --set optimize.rounds=1 --set optimize.runs=4 --set drawing.draw_lanes=1 --set drawing.realise=$r
      done
    done
    note "all done" ;;
esac
