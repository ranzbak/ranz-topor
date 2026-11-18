#!/usr/bin/env bash
# exist-term mutants (Hetzner, inside an hzq unit): exist_term_mut.sh <unit> <mutant>...: copy the job tree, then per mutant
# apply it (tools/exist_term_mut.py), run the targeted release tests (topo_exist_term; the core exist_end unit test when
# those pass), record "killed" / "SURVIVED" in $HZ_RUN_DIR/mut-<unit>.txt, revert.
set -u
R=${HZ_RUN_DIR:-/tmp/xtm}; U=$1; shift
W=/root/xtm/$U; rm -rf "$W"; mkdir -p "$W" "$R"
rsync -a --exclude target --exclude .git --exclude scratch --exclude .venv ./ "$W"/
export CARGO_TARGET_DIR=/root/xtm-target-$U CARGO_BUILD_JOBS=4
O=$R/mut-$U.txt; : > "$O"
for m in base "$@"; do
  [ "$m" = base ] || python3 "$W"/tools/exist_term_mut.py "$m" apply "$W" >> "$O" 2>&1 || { echo "$m apply-failed" >> "$O"; continue; }
  ( cd "$W"/engine && cargo test --release --test topo_exist_term > "$R/mut-$U-$m.log" 2>&1 ); rc=$?
  if [ $rc -eq 0 ]; then ( cd "$W"/engine && cargo test --release --lib exist_end_maps >> "$R/mut-$U-$m.log" 2>&1 ); rc=$?; fi
  fails=$(grep -E '^test .* FAILED$|panicked' "$R/mut-$U-$m.log" | head -3 | tr '\n' ' ')
  if [ "$m" = base ]; then echo "base rc $rc $fails" >> "$O"; elif [ $rc -ne 0 ]; then echo "$m killed rc $rc $fails" >> "$O"; else echo "$m SURVIVED" >> "$O"; fi
  [ "$m" = base ] || python3 "$W"/tools/exist_term_mut.py "$m" revert "$W" >> "$O" 2>&1
done
echo "$U done" >> "$O"
