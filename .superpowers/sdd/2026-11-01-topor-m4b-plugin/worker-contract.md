# Worker contract (topor M4b plugin)

- Project: topor, a KiCad curved-trace topological router. Rust engine engine/, Python harness harness/ (dev/test, pcbnew oracle),
  new shared pure package toporlib/, KiCad IPC plugin plugin/ (package plugin/topor/), tests tests/ (plugin tests tests/plugin/).
- Read first: your task brief (path in your dispatch) = your requirements, exact values verbatim. Then
  /home/paul/work/kicad/plugins/topor/.superpowers/sdd/2026-11-01-topor-m4b-plugin/global-constraints.md
  (plan constraints + main rulings M4-R1..R6 + coordinator rulings C1..C5). Read the spec/design docs the brief names only as far
  as you need them. Never read the whole plan file.
- Work in the directory you were started in (your git worktree). Commit there with the conventional commit message the brief
  gives, ending with the line:  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  NEVER push (no `git push` of any branch, ever — only the main session pushes). Never modify anything under ~/work/kicad/projects. Never write to the user's KiCad config, plugin dir or plugin venv;
  never run `tools/build.sh --dev`; never touch a running KiCad.
- Commands: `cargo test --release --manifest-path engine/Cargo.toml [--test X]`,
  `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings` (clean).
  Python: `/home/paul/work/kicad/plugins/topor/.venv/bin/python` / `/home/paul/work/kicad/plugins/topor/.venv/bin/pytest`
  with cwd = your working directory (pytest.ini puts `.` on sys.path; tests importing `topor` need `plugin` on sys.path —
  add it in tests/plugin/conftest.py if not present).
  ONLY if your dispatch says you own the .venv may you run
  `VIRTUAL_ENV=/home/paul/work/kicad/plugins/topor/.venv /home/paul/work/kicad/plugins/topor/.venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python`.
  Otherwise never modify the .venv (shared).
- TDD: failing tests first, see them fail, implement, see them pass. Focused tests while iterating; the relevant full suite once
  before committing (`.venv/bin/pytest -q` default suite; cargo + clippy if you touched Rust). Never weaken, skip or delete a test or
  a bound to make it pass; never relax a budget/cap. If the spec cannot be met, stop and report BLOCKED / DONE_WITH_CONCERNS with numbers.
- Every new component states its Big-O in a doc comment (45 §5a).
- No subagents: do all the work yourself; never spawn a reviewer.
- Report: write the full report (what changed per file, decisions + why, test commands + summarised output, numbers the brief asks
  for, UNVERIFIED items, concerns) to the report-file path in your dispatch. Final reply SHORT: status
  (DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT), commit hash(es) + branch, one-line test summary, concerns (≤ 5 lines).
- Benchmark/progress PNGs under docs/benchmarks/ are committed with `git add -f` (a local .gitignore change is pending the user's decision; never edit .gitignore).
- M4-R8: IPC model approximations must be conservative (superset obstacles, 'not reached' pour reach, unsure via = fixed) and tests assert that direction.
