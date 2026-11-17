#!/usr/bin/env bash
# fx_sweep_run.sh <tree> <venv> <out>: via_fold variants (tools/fx_sweep_via.py) routed in erbs mode with a probe engine (rev with FXP lines);
# one line per variant: the count of "erbs_x_any_layer differs" lines.
set -u
T=$1; V=$2; O=$3
if [ -z "${DISPLAY:-}" ]; then exec xvfb-run -a bash "$0" "$@"; fi
mkdir -p $O; cd $T
cp $0 /dev/null
$V/bin/python $O/fx_sweep_via.py $O/boards > $O/gen.log 2>&1
for d in $O/boards/*/; do
  n=$(basename $d)
  TOPOR_VENV=$V PATH=$V/bin:$PATH tools/route_board.sh $d/problem/$n.kicad_pcb $O/run-$n --no-png --timeout 120 --set optimize.rounds=1 --set drawing.realise=erbs > $O/$n.out 2> $O/$n.err
  echo "$n differs $(grep -c 'FXP erbs_x_any_layer differs' $O/$n.err) routed $(grep -h 'routed ' $O/$n.out | head -1)" >> $O/result.txt
  rm -rf $O/run-$n
done
echo done >> $O/result.txt
