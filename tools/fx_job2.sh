#!/usr/bin/env bash
# job entry: two hzq units (4 threads each): the mutant runs of the new tests, and the full release suite
set -u
T=$(pwd); R=$HZ_RUN_DIR
hzq run --threads 4 --name fx-mut-tests -- bash tools/fx_mut_tests.sh $R $T &
hzq run --threads 4 --name fx-full -- bash tools/fx_full.sh $R $T &
wait
