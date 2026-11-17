#!/usr/bin/env bash
# edge_merge.sh [main-rev]: merge trial replay for edge-clear in the CURRENT (throw-away) worktree on the branch tip.
# Known conflict (main beeae65, fx-survivors merged): harness/fx_survivors_png.py add/add; the branch version is main's
# plus the TOPOR_FX_REV / TOPOR_FX_STATUS labels (defaults = main's old text), so take the branch side.
set -eu
git config rerere.enabled true
if ! git merge --no-edit "${1:-main}"; then
  git checkout --ours harness/fx_survivors_png.py && git add harness/fx_survivors_png.py
  left=$(git diff --name-only --diff-filter=U)
  [ -z "$left" ] || { echo "unresolved: $left"; exit 1; }
  git commit --no-edit
fi
git log --oneline -1
