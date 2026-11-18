#!/usr/bin/env bash
# tw4 T3 mutants (dangling_lead_choice): each mutant in its own copy of the engine (own target dir), the T3 tests
# must fail on it. Usage: tools/tw4_t3_mut.sh <out-dir> [mutant...]   (default: all)
set -u
OUT=${1:?out dir}; shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
W=${T3_MUT_TREE:-$OUT/tree}; mkdir -p "$OUT" "$W"
rsync -a --delete --exclude target "$ROOT/engine/" "$W/engine/"
cp "$ROOT/Cargo.toml" "$ROOT/Cargo.lock" "$W/"   # the workspace root (else cargo finds the repo's)
export CARGO_TARGET_DIR=${T3_MUT_TARGET:-$OUT/target}
TESTS="--test topo_exist_term"
declare -A F P R
F[pts_off]=src/topo/board.rs;  P[pts_off]='if o.lead_choice { kept_ends.iter()';  R[pts_off]='if false \&\& o.lead_choice { kept_ends.iter()'
F[retain_end]=src/topo/core.rs; P[retain_end]='Some(alt) => ends.retain(|e| alt.iter().any(|x| x.0 == e.v)),'; R[retain_end]='Some(_alt) => ends.retain(|e| e.v == v),'
F[draw_full]=src/topo/draw.rs;  P[draw_full]='let q = if k.choices.is_empty() { q } else {'; R[draw_full]='let q = if true { q } else {'
F[via_past]=src/topo/draw.rs;   P[via_past]='.is_some_and(|t| (ve.0 - k.at).len() > t + 1.0)'; R[via_past]='.is_some_and(|t| false \&\& (ve.0 - k.at).len() > t + 1.0)'
F[octi_dir]=src/topo/octi.rs;   P[octi_dir]='// tw4 T3: a run from a start point short of the lead'"'"'s end goes on from the lead\n                Some(d)'; R[octi_dir]=''
F[no_turn]=src/topo/core.rs;    P[no_turn]='cont_dirs.insert(u, \*d);'; R[no_turn]='let _ = (u, d);'
F[centre_far]=src/topo/state.rs; P[centre_far]='(proj.pos\[v\] - c).len() < (proj.pos\[u\] - c).len()'; R[centre_far]='(proj.pos[v] - c).len() > (proj.pos[u] - c).len()'
MS=("$@"); [ ${#MS[@]} -eq 0 ] && MS=(pts_off retain_end draw_full via_past octi_dir no_turn centre_far)
for m in "${MS[@]}"; do
  f=$W/engine/${F[$m]}
  cp "$f" "$f.orig"
  if [ "$m" = octi_dir ]; then
    python3 - "$f" <<'PY'
import sys; p=sys.argv[1]; s=open(p).read()
old="                // tw4 T3: a run from a start point short of the lead's end goes on from the lead\n                Some(d)"
assert old in s; open(p,'w').write(s.replace(old, "                None"))
PY
  else
    sed -i "s/${P[$m]}/${R[$m]}/" "$f"
  fi
  if cmp -s "$f" "$f.orig"; then echo "mutant $m NOT APPLIED" >> "$OUT/mutants.txt"; continue; fi
  touch "$f"
  ( cd "$W/engine" && cargo test --release $TESTS --lib -- a_lead_offers a_lead_that_points a_start_short > "$OUT/mut-$m.log" 2>&1 )
  rc=$?
  grep -q "^test result" "$OUT/mut-$m.log" || rc="build-failed($rc)"
  echo "mutant $m rc $rc ($( [ "$rc" = 0 ] && echo SURVIVED || echo killed )): $(grep -E '^test .* FAILED|panicked' "$OUT/mut-$m.log" | head -3 | tr '\n' ' ')" >> "$OUT/mutants.txt"
  mv "$f.orig" "$f"; touch "$f"
done
echo "all done" >> "$OUT/mutants.txt"
