set -u
E=scratch/boards/esp-rust-board/4907cec7fec3/esp-rust-board.kicad_pcb
Q=scratch/boards/qmtech-hand/qmtech-minimig.kicad_pcb
A="--set optimize.rounds=1 --set optimize.runs=1"
R=$HZ_RUN_DIR
u() { n=$1; shift; ( hzq run --threads 4 --name $n -- "$@" > $R/$n.log 2>&1; echo "$n rc $?" >> $R/rc.txt ) & }
u bh0 python -m harness.score_runs $R/bh0 bornhack2026 --only 0 $A
u qm0 python -m harness.score_runs $R/qm0 $Q --only 0 $A
u esp0 python -m harness.score_runs $R/esp0 $E --only 0 $A
u eprom python -m harness.score_runs $R/eprom eprom-to-kernal --only 0 $A
u wiccon python -m harness.score_runs $R/wiccon wiccon-2023 --only 0 $A
wait; echo "all done" >> $R/rc.txt
