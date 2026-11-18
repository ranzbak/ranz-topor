#!/usr/bin/env bash
# stable-label r5: replay of the merge of main bc42ab0 into stable-label f140fb5 (rule 11) in a throw-away worktree.
# usage: tools/sl5_merge_replay.sh <new-worktree-dir> [base=f140fb5] [main=bc42ab0]
# rerere (shared rr-cache of the repo) replays the core.rs / run.py resolutions; the optimise.rs signature fix
# (limit-wait's three optimise() tests get stable-label's on_pick argument) is applied by sed. Then cargo check.
set -eu
WT=$1; BASE=${2:-f140fb5}; MAIN=${3:-bc42ab0}
git worktree add --detach "$WT" "$BASE"
cd "$WT"
git config rerere.enabled true
git merge --no-commit "$MAIN" || true
git rerere
sed -i -E '/fn past_the_grace|fn a_stop_the_cycle_saw/,/^    }$/ s/(optimise\(&mut sp, &mut a, &params\(1, 0\), [^;]*&\|\| 0\.0)\);/\1, \&mut |_, _, _| {});/' engine/src/topo/optimise.rs
if git diff --name-only --diff-filter=U | grep -q .; then git diff --name-only --diff-filter=U; echo "unresolved conflicts"; exit 1; fi
grep -c '&mut |_, _, _| {})' engine/src/topo/optimise.rs
cargo check --release --all-targets --manifest-path engine/Cargo.toml
