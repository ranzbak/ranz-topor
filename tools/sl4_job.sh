#!/usr/bin/env bash
# stable-label r4 (fix round): targeted cargo release + targeted pytest + mutants of the changed lines
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
u crel 4 bash tools/sl4_rel.sh "$R"
u py 4 bash tools/sl4_py.sh "$R"
u mutr 4 python3 tools/sl4_mut.py "$R" rust
u mutp 4 python3 tools/sl4_mut.py "$R" py
wait
echo "all done" >> "$R/rc.txt"
