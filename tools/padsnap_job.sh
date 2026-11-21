#!/usr/bin/env bash
# pad-snap r1 (A1b) on the VM, one job per rev (run from the rev's tree root):
#   tools/padsnap_job.sh <main venv> <base qmtech run dir> <base esp run dir>      (the job of the tip rev)
# The base boards run in their own jobs at the main rev (score_runs, one unit each, same options). Units here: qmtech +
# esp det seed 0 (score_runs --only 0, optimize.rounds=1, runs=1; 4 threads each), the pattern pad_end_centre at the
# tip and at main (main's engine from <main venv>, this tree's harness; generated, routed, kicad-cli DRC with warnings,
# Cleanup replay, off-centre counts), the mutants (tools/padsnap_mut.py on a tree copy); then the off-centre counts and
# DRC with warnings of the base boards and the pattern PNG (setup | expected | main | tip).
# Internal unit modes: tools/padsnap_job.sh unit <board|pattern|mut|counts> <label> ...
set -u
R=${HZ_RUN_DIR:?}; ROOT=$PWD
note() { echo "$*" >> $R/rc.txt; }
A=(--no-png --set optimize.rounds=1 --set optimize.runs=1)
if [ "${1:-}" = unit ]; then
  kind=$2; L=$3
  case $kind in
    board)  # $4 name, $5 pcb
      python -m harness.score_runs $R/$L/$4 $5 --only 0 "${A[@]}" > $R/$L-$4.log 2>&1 9>&-; note "$L-$4 rc $?"
      pcb=$(ls $R/$L/$4/*/s0/*-topor/*.kicad_pcb 2>/dev/null | head -1)
      [ -n "$pcb" ] && python tools/padsnap_terminals.py "$pcb" $R/$L-$4-terminals.json >> $R/rc.txt 2>&1 9>&-
      [ -n "$pcb" ] && python tools/padsnap_drc_all.py "$pcb" $R/$L-$4-drc-all.json >> $R/rc.txt 2>&1 9>&- ;;
    pattern)  # $4 = engine site dir ("" = this venv's engine)
      B=$R/$L-pboards; P=$R/$L-pruns
      python -m harness.drc_patterns $B pad_end_centre --drc > $R/$L-pattern.log 2>&1 9>&-
      LABEL=$L CFGS=default bash tools/drc_patterns_run.sh $B $P "${4:-}" pad_end_centre >> $R/$L-pattern.log 2>&1 9>&-
      pcb=$P/$L-default/pad_end_centre/pad_end_centre-topor/pad_end_centre.kicad_pcb
      python tools/padsnap_terminals.py $pcb $R/$L-pattern-terminals.json >> $R/rc.txt 2>&1 9>&-
      python -m harness.cleanup_check $pcb --json $R/$L-pattern-cleanup.json >> $R/$L-pattern.log 2>&1 9>&-
      python tools/padsnap_drc_all.py $pcb $R/$L-pattern-drc-all.json >> $R/rc.txt 2>&1 9>&-
      note "$L-pattern done" ;;
    mut)
      d=/root/mt-$(basename $R); mkdir -p $d; rsync -a --exclude target --exclude .venv $ROOT/ $d/ 9>&-
      (cd $d && CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/root/mt-target-pad-snap python tools/padsnap_mut.py $d none M0 M1 M2 M3 M4 M5 M6 M7 M8 M9 M10 > $R/mut.log 2>&1 9>&-)
      note "mutants done" ;;
    counts)  # $4 name, $5 base run dir: off-centre counts + DRC with warnings of a base board
      pcb=$(ls $5/base/$4/*/s0/*-topor/*.kicad_pcb 2>/dev/null | head -1)
      [ -n "$pcb" ] && python tools/padsnap_terminals.py "$pcb" $R/$L-$4-terminals.json >> $R/rc.txt 2>&1 9>&-
      [ -n "$pcb" ] && python tools/padsnap_drc_all.py "$pcb" $R/$L-$4-drc-all.json >> $R/rc.txt 2>&1 9>&-
      note "$L-$4 counts done" ;;
  esac
  exit 0
fi
if [ "${1:-}" = tests ]; then  # tools/padsnap_job.sh tests [mut ids...]: the targeted tests at this rev, then the given mutants (tree copy)
  ( cd engine && CARGO_TARGET_DIR=/root/mt-target-pad-snap cargo test --release --test topo_pad_snap > $R/tests.log 2>&1 9>&-; note "tests rc $?"
    CARGO_TARGET_DIR=/root/mt-target-pad-snap cargo test --release --lib -- stub_price leave_point off_centre_pad_ends forced_ octilinear pad_stub_run >> $R/tests.log 2>&1 9>&-; note "lib tests rc $?" )
  shift
  if [ $# -gt 0 ]; then
    d=/root/mt-$(basename $R); mkdir -p $d; rsync -a --exclude target --exclude .venv $ROOT/ $d/ 9>&-; find $d/engine -name '*.rs' -exec touch {} +
    (cd $d && CARGO_TARGET_DIR=/root/mt-target-pad-snap-m python tools/padsnap_mut.py $d "$@" > $R/mut.log 2>&1 9>&-); note "mutants done"
  fi
  [ -z "${HZQ_UNIT:-}${IN_R2B:-}" ] && note "all done"; exit 0
fi
if [ "${1:-}" = r2b ]; then  # tools/padsnap_job.sh r2b: clippy + targeted tests + mutants, qmtech + esp det s0, the pattern
  ( hzq run --threads 4 --name ps2b-qm -- bash tools/padsnap_job.sh unit board tip qmtech-minimig scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb ) &
  sleep 2
  ( hzq run --threads 4 --name ps2b-esp -- bash tools/padsnap_job.sh unit board tip esp-rust-board scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb ) &
  sleep 2
  ( hzq run --threads 1 --name ps2b-pat -- bash tools/padsnap_job.sh unit pattern tip ) &
  sleep 2
  ( hzq run --threads 4 --name ps2b-tm -- bash -c "rustup component add clippy >/dev/null 2>&1; cd engine && CARGO_BUILD_JOBS=4 CARGO_TARGET_DIR=/root/mt-target-pad-snap cargo clippy --release --all-targets > $R/clippy.log 2>&1 9>&-; echo \"clippy rc \$?\" >> $R/rc.txt; cd .. && IN_R2B=1 CARGO_BUILD_JOBS=4 bash tools/padsnap_job.sh tests none M13" ) &
  wait
  find $R -type f ! -name '*.json' ! -name '*.log' ! -name '*.txt' ! -name '*.png' ! -path '*-topor/*.kicad_pcb' ! -name REV ! -name cmd.sh ! -name wrap.sh ! -name log ! -name start ! -name end -delete 2>/dev/null
  find $R -path '*/work/*' -delete 2>/dev/null; find $R -name 'preview*' -delete 2>/dev/null; find $R -name 'result.json' -size +2M -delete 2>/dev/null
  note "all done"; exit 0
fi
if [ "${1:-}" = r2 ]; then  # tools/padsnap_job.sh r2 [mut]: targeted tests, qmtech + esp tip with the debug lines, the pattern + PNG vs main
  MV=/root/venv-33f639987d60; [ -d $MV ] || MV=""
  ( cd engine && CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/root/mt-target-pad-snap cargo test --release --test topo_pad_snap > $R/tests.log 2>&1; note "tests rc $?"
    CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/root/mt-target-pad-snap cargo test --release --lib -- stub_price off_centre_pad_ends forced_ octilinear pad_stub_run >> $R/tests.log 2>&1; note "lib tests rc $?" ) &
  export TOPO_OCTIDBG=1 TOPO_PADSNAPDBG=1
  [ "${2:-}" = dbg ] && export TOPO_VIADBG=1 TOPO_JREL=1 TOPO_FUSEDBG=1
  ( hzq run --threads 4 --name ps2-qm -- bash tools/padsnap_job.sh unit board tip qmtech-minimig scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb ) &
  sleep 2
  ( hzq run --threads 4 --name ps2-esp -- bash tools/padsnap_job.sh unit board tip esp-rust-board scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb ) &
  sleep 2
  unset TOPO_OCTIDBG TOPO_PADSNAPDBG TOPO_VIADBG TOPO_JREL TOPO_FUSEDBG
  ( hzq run --threads 1 --name ps2-pat -- bash tools/padsnap_job.sh unit pattern tip ) &
  wait
  if [ "${2:-}" = mut ]; then bash tools/padsnap_job.sh unit mut tip; fi
  # r2b dbg: the debug lines only (the full logs are large)
  if [ "${2:-}" = dbg ]; then for f in $(find $R/tip -name '*-s0.log'); do
    grep -E 'FORCEVIA|VIAPICK|VIAMISS|PADOFF|PADSTUB|JREL|FUSE|OCTIFORCEDBG|OCTISKIP|^OCTI |PULL wire|never drop|branch points' $f > $f.dbg.log; rm -f $f; done; fi
  # slow uplink: keep only metrics, digests, logs and the routed copper for the fetch
  find $R -type f ! -name '*.json' ! -name '*.log' ! -name '*.txt' ! -name '*.png' ! -path '*-topor/*.kicad_pcb' ! -name REV ! -name cmd.sh ! -name wrap.sh ! -name log ! -name start ! -name end -delete 2>/dev/null
  find $R -path '*/work/*' -delete 2>/dev/null; find $R -name 'preview*' -delete 2>/dev/null; find $R -name 'result.json' -size +2M -delete 2>/dev/null
  note "all done"; exit 0
fi
if [ "${1:-}" = pattern-only ]; then  # tools/padsnap_job.sh pattern-only <main venv>: the pattern (tip + main), mutants, PNG
  MS=$R/mainsite; mkdir -p $MS; ln -sfn $(ls -d $2/lib/python3*/site-packages/topor_engine | head -1) $MS/topor_engine
  ( hzq run --threads 1 --name ps-pat -- bash -c "bash tools/padsnap_job.sh unit pattern tip; bash tools/padsnap_job.sh unit pattern base $MS" ) &
  sleep 2
  ( hzq run --threads 4 --name ps-mut -- bash tools/padsnap_job.sh unit mut tip ) &
  wait
  mkdir -p $R/png-runs; cp -r $R/base-pruns/base-default $R/png-runs/main-default; cp -r $R/tip-pruns/tip-default $R/png-runs/tip-default
  python -m harness.drc_patterns_png $R/png-runs $R/png pad_end_centre > $R/png.log 2>&1 9>&-; note "png rc $?"
  note "all done"; exit 0
fi
MV=${1:?}; BQ=${2:?}; BE=${3:?}
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
MS=$R/mainsite; mkdir -p $MS; ln -sfn $(ls -d $MV/lib/python3*/site-packages/topor_engine | head -1) $MS/topor_engine
( hzq run --threads 4 --name ps-tip-qm -- bash tools/padsnap_job.sh unit board tip qmtech-minimig $Q ) &
sleep 2
( hzq run --threads 4 --name ps-tip-esp -- bash tools/padsnap_job.sh unit board tip esp-rust-board $E ) &
sleep 2
( hzq run --threads 1 --name ps-pat -- bash -c "bash tools/padsnap_job.sh unit pattern tip; bash tools/padsnap_job.sh unit pattern base $MS" ) &
sleep 2
( hzq run --threads 4 --name ps-mut -- bash tools/padsnap_job.sh unit mut tip ) &
wait
for d in $BQ $BE; do for i in $(seq 120); do [ -f $d/exit_code ] && break; sleep 20; done; done
bash tools/padsnap_job.sh unit counts base qmtech-minimig $BQ
bash tools/padsnap_job.sh unit counts base esp-rust-board $BE
mkdir -p $R/png-runs
cp -r $R/base-pruns/base-default $R/png-runs/main-default
cp -r $R/tip-pruns/tip-default $R/png-runs/tip-default
python -m harness.drc_patterns_png $R/png-runs $R/png pad_end_centre > $R/png.log 2>&1 9>&-; note "png rc $?"
note "all done"
