#!/usr/bin/env bash
# pardraw_job.sh (branch par-draw, Hetzner): phase A acceptance.
#   all : par tests (lib par::, tests/par_draw at --test-threads 1), config pytest, clippy; board copper hashes
#         (eprom/wiccon/esp/qmtech x realise sequential/erbs x RAYON 1/2/4/8, rounds=1 runs=1; eprom/wiccon erbs
#         with the default runs at RAYON 1/4/8); then the completion-order mutant, which must fail the tests.
#   ref : the board hashes of this rev at RAYON 4 only (run it on main for "copper = main").
#   time: realise = erbs, qmtech + esp at RAYON 1 and 8 (TOPO_TIMES; exclusive VM).
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
    ( hzq run --threads 4 --name pd-tests -- bash -c "cd $ROOT/engine && RAYON_NUM_THREADS=4 cargo test --release --lib par:: 9>&- > $R/tests.log 2>&1; echo \"lib rc \$?\" >> $R/tests.log; RAYON_NUM_THREADS=4 cargo test --release --test par_draw -- --test-threads 1 9>&- >> $R/tests.log 2>&1; echo \"par_draw rc \$?\" >> $R/tests.log; RAYON_NUM_THREADS=4 cargo test --release --test par_refine --test par_refine_dense --test topo_drawing_grace --test topo_m3_class_keep 9>&- >> $R/tests.log 2>&1; echo \"near rc \$?\" >> $R/tests.log; cargo clippy --release --all-targets 9>&- >> $R/tests.log 2>&1; echo \"clippy rc \$?\" >> $R/tests.log; cd $ROOT && CI=1 python -m pytest -q tests/plugin/test_config.py tests/plugin/test_config_docs.py tests/plugin/test_toml_defaults.py tests/test_docs_links.py >> $R/tests.log 2>&1; echo \"pytest rc \$?\" >> $R/tests.log" ) & sleep 1
    boards 1 2 4 8
    for t in 1 4 8; do
      one eprom-def-t$t $P $t --set drawing.realise=erbs & sleep 1
      one wiccon-def-t$t $W $t --set drawing.realise=erbs & sleep 1
    done
    wait
    grep -E "rc [0-9]|^test result|FAILED|panicked|^(warning|error)(\[|:)" $R/tests.log | while read -r l; do note "tests: $l"; done
    # mutant: the lanes' results in completion order (the fold no longer sees unit order)
    cd $ROOT/engine
    python3 - <<'EOF'
p = "src/topo/par.rs"
s = open(p).read()
a = "    let next = AtomicUsize::new(0);\n"
b = "        let r = unit(i);\n"
assert s.count(a) == 1 and s.count(b) == 1
s = s.replace(a, a + "    let done = AtomicUsize::new(0);\n").replace(b, b + "        let i = done.fetch_add(1, Ordering::Relaxed);\n")
open(p, "w").write(s)
EOF
    hzq run --threads 4 --name pd-mut -- bash -c "RAYON_NUM_THREADS=4 cargo test --release --lib par:: 9>&- > $R/mutant.log 2>&1; echo \"lib rc \$?\" >> $R/mutant.log; RAYON_NUM_THREADS=4 cargo test --release --test par_draw -- --test-threads 1 9>&- >> $R/mutant.log 2>&1; echo \"par_draw rc \$?\" >> $R/mutant.log"
    grep -E "rc [0-9]|^test result" $R/mutant.log | while read -r l; do note "mutant (must fail): $l"; done
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
  time)
    for t in 1 8; do
      for bb in qmtech:$Q esp:$E; do
        n=${bb%%:*}; b=${bb#*:}
        one $n-erbs-t$t $b $t --set optimize.rounds=1 --set optimize.runs=1 --set drawing.realise=erbs
      done
    done
    note "all done" ;;
esac
