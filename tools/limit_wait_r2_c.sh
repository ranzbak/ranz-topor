#!/usr/bin/env bash
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
( hzq run --threads 4 --name rel -- bash tools/limit_wait_cargo.sh "$R" rel > "$R/rel.log" 2>&1; echo "rel rc $?" >> "$R/rc.txt" ) &
wait
echo "all done" >> "$R/rc.txt"
