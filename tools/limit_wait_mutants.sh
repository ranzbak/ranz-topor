#!/usr/bin/env bash
# limit-wait r2 mutants: each mutant in its own copy; usage: limit_wait_mutants.sh <outdir> <mutant>...
R=$1; shift
for m in "$@"; do
  W=/root/lw2-mut-$m; python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$W"; mkdir -p "$W"
  tar --exclude=./target --exclude=./scratch --exclude=./.git -cf - . | tar -xf - -C "$W"
  cp tools/limit_wait_mut.py "$W/mut.py"; (cd "$W" && python3 mut.py $m)
  (cd "$W" && CARGO_TARGET_DIR=/root/lw2-tgt-$m cargo test --release --manifest-path engine/Cargo.toml --lib --test topo_limit_wait --no-fail-fast -- wait_line stay topo_limit_wait 2>&1 | grep -E "^test .*FAILED|test result" > "$R/mut-$m.txt")
  echo "mutant $m: $(grep -c FAILED $R/mut-$m.txt) failed" >> "$R/mutants.txt"
done
