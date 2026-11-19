#!/usr/bin/env bash
# no-finished-notice fix round 2: full pytest (Python touched) + mutants in own tree copies, parallel hzq units
R=$HZ_RUN_DIR
hzq run --pin --threads 1 --name nofin2-pytest -- bash tools/nofin_r2_unit.sh pytest &
hzq run --pin --threads 1 --name nofin2-mutA -- bash tools/nofin_r2_unit.sh mut M1 M2 M3 M4 M4b &
hzq run --pin --threads 1 --name nofin2-mutB -- bash tools/nofin_r2_unit.sh mut M5 M6 M7 A1 A2 &
hzq run --pin --threads 1 --name nofin2-mutC -- bash tools/nofin_r2_unit.sh mut A3 B1 B2 B3 B4 &
wait
{ for f in $R/mut-*.txt; do cat $f; done; tail -4 $R/pytest.log; } > $R/rc.txt; echo "all done" >> $R/rc.txt
