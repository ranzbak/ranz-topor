#!/usr/bin/env bash
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
hzq run --threads 4 --name mut2 -- bash tools/limit_wait_mutants_all.sh default_off > "$R/mut2.log" 2>&1
echo "all done" >> "$R/rc.txt"
