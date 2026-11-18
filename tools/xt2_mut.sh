#!/usr/bin/env bash
# exist-term r2 mutants (Hetzner, inside an hzq unit): xt2_mut.sh <unit> <mutant>...: copy the job tree, then per mutant
# apply it (tools/xt2_mut.py), run the branch tests of the round, record "killed" / "SURVIVED" in $HZ_RUN_DIR/mut-<unit>.txt,
# revert. Rust mutants: topo_exist_term + the lib tests board::tests, tidy::tests, unsupported_ends; py_* mutants: the two
# pytest tests of the round (no rebuild: the engine is unchanged).
set -u
R=${HZ_RUN_DIR:-/tmp/xt2m}; U=$1; shift
J=$(basename "$R"); X=${XT2M_ROOT:-/root}; W=$X/xt2m/$J-$U; mkdir -p "$W" "$R"   # per job + unit: two jobs never share a tree (XT2M_ROOT: local runs)
rsync -a --delete --exclude target --exclude .git --exclude .venv ./ "$W"/
# rsync keeps the sources' mtimes, older than a reused target's last build: cargo would test a stale build (r4: the base
# ran the previous run's last mutant). Touch them so the base rebuilds.
find "$W" -name '*.rs' -exec touch {} +
export CARGO_TARGET_DIR=${XT2M_TARGET:-$X/xt2m-target-$J-$U} CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} RUST_TEST_THREADS=${RUST_TEST_THREADS:-2}
O=$R/mut-$U.txt; : > "$O"
rs() { ( cd "$W"/engine && cargo test --release --test topo_exist_term && cargo test --release --lib -- topo::board::tests topo::tidy::tests unsupported_ends exist_end_maps geom::prim ); }
py() { ( cd "$W" && CI=1 python -m pytest -q -x tests/test_drcloop.py -k remove_uuids && CI=1 python -m pytest -q -x tests/plugin/test_run_flow.py -k replaced ); }
for m in base "$@"; do
  [ "$m" = base ] || python3 "$W"/tools/xt2_mut.py "$m" apply "$W" >> "$O" 2>&1 || { echo "$m apply-failed" >> "$O"; continue; }
  k=$m; [ "$m" = base ] && k=${1:-base}   # the base runs the tests the unit's mutants need
  case $k in py_*) py > "$R/mut-$U-$m.log" 2>&1 ;; *) rs > "$R/mut-$U-$m.log" 2>&1 ;; esac; rc=$?
  fails=$(grep -E '^test .* FAILED$|panicked at|^FAILED ' "$R/mut-$U-$m.log" | head -3 | tr '\n' ' ')
  if [ "$m" = base ]; then echo "base rc $rc $fails" >> "$O"; elif [ $rc -ne 0 ]; then echo "$m killed rc $rc $fails" >> "$O"; else echo "$m SURVIVED" >> "$O"; fi
  [ "$m" = base ] || python3 "$W"/tools/xt2_mut.py "$m" revert "$W" >> "$O" 2>&1
done
echo "$U done" >> "$O"
