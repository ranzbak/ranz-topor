#!/usr/bin/env bash
# stable-label r4b: targeted cargo release + rust mutants (incl. the pre-fix core.rs) after the test fix
set -u
R=$HZ_RUN_DIR; mkdir -p "$R"
u() { n=$1; th=$2; shift 2; ( hzq run --threads "$th" --name "$n" -- "$@" > "$R/$n.log" 2>&1; echo "$n rc $?" >> "$R/rc.txt" ) & }
u crel 4 bash tools/sl4_rel.sh "$R"
u mutr 4 python3 tools/sl4_mut.py "$R" rust
wait
echo "all done" >> "$R/rc.txt"
