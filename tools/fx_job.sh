#!/usr/bin/env bash
# job entry: the PNG run (patterns x seq/erbs on the engine of this rev = main 0a53849's) then the mutant runs
set -u
bash tools/fx_units.sh "$HZ_RUN_DIR/png-run"
bash tools/fx_mut.sh "$HZ_RUN_DIR/mut-run"
