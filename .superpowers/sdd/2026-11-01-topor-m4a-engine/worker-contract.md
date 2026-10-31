# Worker contract (topor M4a engine 0.5.0)

- Project: topor, a KiCad curved-trace topological router. Rust engine engine/, Python harness harness/ (dev/test, pcbnew oracle),
  shared pure package toporlib/, KiCad IPC plugin plugin/ (package plugin/topor/), tests tests/ (plugin tests tests/plugin/).
- Read first: your task brief (path in your dispatch) = your requirements, exact values verbatim. Then
  /home/paul/work/kicad/plugins/topor/.superpowers/sdd/2026-11-01-topor-m4a-engine/global-constraints.md
  (plan constraints + main rulings + coordinator rulings A-C*). Read spec/design docs the brief names only as far as you need them.
  Never read the whole plan file (extract your task with: awk '/^### Task <ID> /,/^### Task [^<ID>]/' — the brief already has it).
- Work in the directory you were started in (your git worktree). Commit there with the conventional commit message the brief
  gives, ending with the line:  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  NEVER push (no `git push` of any branch, ever — only the main session pushes). Never modify anything under ~/work/kicad/projects
  (boards are read ONLY from scratch/boards/<board>/<sha12>/ snapshots via harness.paths). Never write to the user's KiCad config,
  plugin dir or plugin venv; never run `tools/build.sh --dev`; never touch a running KiCad.
- Worktree setup (once): `ln -s /home/paul/work/kicad/plugins/topor/scratch scratch` (scratch/ is gitignored; read the snapshots,
  write your own outputs under target/ or /tmp, never into the shared scratch/boards).
- Commands: `cargo test --release --manifest-path engine/Cargo.toml [--test X]`,
  `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings` (clean).
  Python: `/home/paul/work/kicad/plugins/topor/.venv/bin/python` / `.../.venv/bin/pytest` with cwd = your worktree.
  **The shared .venv belongs to the coordinator (Ruling A-C2): never run maturin develop / pip.** To use YOUR engine build from Python:
  ```
  cargo build --release --features python --manifest-path engine/Cargo.toml
  mkdir -p target/pyeng/topor_engine
  cp /home/paul/work/kicad/plugins/topor/.venv/lib/python3.12/site-packages/topor_engine/__init__.py target/pyeng/topor_engine/
  cp target/release/libtopor_engine.so target/pyeng/topor_engine/topor_engine.abi3.so
  export PYTHONPATH=$PWD/target/pyeng            # prepend to every python/pytest command
  python -c "import topor_engine; print(topor_engine.__file__)"   # must print target/pyeng/...
  ```
  Repeat the build + copy after every Rust change before any engine-using pytest / harness / progress run.
- TDD: failing tests first, see them fail, implement, see them pass. Focused tests while iterating; the relevant full suite once
  before committing (`.venv/bin/pytest -q` default suite with PYTHONPATH set; cargo + clippy if you touched Rust). Never weaken, skip
  or delete a test or a bound to make it pass; never relax a budget/cap/gate number. If the spec cannot be met, stop and report
  BLOCKED / DONE_WITH_CONCERNS with numbers.
- Every new component states its Big-O in a doc comment (45 §5a); every new loop is capped and checks deadline + cancel (`Ctx::stop`).
- Determinism: no HashMap/HashSet iteration order may reach the output; threads 1/2/4 + repeat byte-identical.
- Judgement calls become Options fields with sane defaults (and, if user-facing, a plugin/topor/schema.py Setting) — "if in doubt,
  make it configurable"; name each one in your report.
- No subagents: do all the work yourself; never spawn a reviewer.
- Progress PNG (outcome-affecting tasks): `PYTHONPATH=$PWD/target/pyeng .venv/bin/python -m harness.progress --milestone m4 --task <ID>
  --router topor --board <board>` writes docs/benchmarks/progress/<date>-m4-<ID>-<board>.png in YOUR worktree. docs/benchmarks/ is
  gitignored and untracked by user ruling: NEVER commit anything under docs/benchmarks/ (no `git add -f`). Put the absolute PNG path +
  the caption numbers (routed/total, vias, routing_new, preexisting, unconnected, router_s, detour line, pcb sha12) in your report;
  the coordinator copies the file. Never present a test pattern/fixture image as a result.
- Report: write the full report (what changed per file, decisions + why, test commands + summarised output, numbers the brief asks
  for, concerns) to `.superpowers/sdd/2026-11-01-topor-m4a-engine/task-<ID>-report.md` INSIDE your worktree and commit it
  (`git add -f` that one file). Final reply SHORT: status (DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT), commit hash(es) +
  branch, one-line test summary, key numbers, concerns (≤ 5 lines).
