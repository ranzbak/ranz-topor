#!/usr/bin/env bash
# pick-necks r2: multi-row tables (rounds 8; qm 4) so the four bests differ; targeted pytest.
set -u
OUT=$HZ_RUN_DIR; TAG=${HZ_TAG:-pkn2}
EP=scratch/boards/eprom-to-kernal/bfaf705c0f25/eprom-to-kernal.kicad_pcb; WI=scratch/boards/wiccon-2023/2d3063e70ed5/2023.kicad_pcb
QM=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb; ESP=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
unit() { local a=$1 n=$2 p=$3; shift 3; TOPOR_UNRELEASED_SETTINGS=1 python -m harness.score_runs "$OUT/$a-$n" "$p" --only 0 --no-png --threads ${TH:-4} "$@" > "$OUT/$a-$n.log" 2>&1 9>&-; echo "$a-$n rc $?" >> "$OUT/rc.txt"; }
small() { C="--set optimize.runs=1 --set optimize.rounds=8 --set capacity.compass_cut_check=true"; TH=2 unit r8 ep $EP $C & TH=2 unit r8 wi $WI $C & TH=2 unit r8 esp $ESP $C & wait
  (CI=1 python -m pytest -q tests/plugin/test_report.py tests/plugin/test_config_docs.py > $OUT/pytest.log 2>&1 9>&-; echo "pytest rc $?" >> $OUT/rc.txt); }
qm() { unit r4 qm $QM --set optimize.runs=1 --set optimize.rounds=4 --set capacity.compass_cut_check=true; }
export -f unit small qm; export OUT EP WI QM ESP
hzq run --threads 6 --name $TAG-small --pin -- bash -c 'small' 9>&- &
sleep 3
hzq run --threads 4 --name $TAG-qm --pin -- bash -c 'qm' 9>&- &
wait
echo "all done" >> $OUT/rc.txt
