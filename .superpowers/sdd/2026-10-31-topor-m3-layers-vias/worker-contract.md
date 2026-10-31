# Worker contract (topor M3)

- Project: topor, a KiCad curved-trace topological router. Rust engine in engine/, Python harness in harness/, tests in tests/.
- Read first: your task brief (path in your dispatch) = your requirements, exact values verbatim. Then
  /home/paul/work/kicad/plugins/topor/.superpowers/sdd/2026-10-31-topor-m3-layers-vias/global-constraints.md (binding constraints + main rulings).
  Read the spec/design docs the brief names only as far as you need them. Never read the whole plan file.
- Work in the directory you were started in (your git worktree, or the main repo if told so). Commit there with a conventional
  commit message (the brief gives it) ending with the line:  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  NEVER push. Never modify anything under ~/work/kicad/projects (read-only source boards; the harness copies them).
- Build/test commands: `cargo test --release --manifest-path engine/Cargo.toml [--test X]`,
  `cargo clippy --all-targets --manifest-path engine/Cargo.toml -- -D warnings` (must be clean).
  Python: `/home/paul/work/kicad/plugins/topor/.venv/bin/python` / `.venv/bin/pytest` with cwd = your working directory.
  ONLY if your dispatch says you own the .venv may you run
  `VIRTUAL_ENV=/home/paul/work/kicad/plugins/topor/.venv /home/paul/work/kicad/plugins/topor/.venv/bin/maturin develop --release --manifest-path engine/Cargo.toml --features python`
  or tools/build.sh. Otherwise do not touch the .venv (other workers share it).
- TDD: write the failing tests first, see them fail, implement, see them pass. Run the focused tests while iterating; the full engine
  suite + clippy once before committing. Never weaken, skip or delete a test or a bound to make it pass; never relax a budget/cap.
  If the spec cannot be met, stop and report BLOCKED / DONE_WITH_CONCERNS with numbers.
- Every new component: state its Big-O in a doc comment (45 §5a).
- No subagents: do all the work yourself; never spawn a reviewer.
- Report: write the full report (what changed per file, decisions you made and why, test commands + their summarised output,
  numbers asked for by the brief, concerns) to the report-file path in your dispatch. Your final reply is SHORT: status
  (DONE | DONE_WITH_CONCERNS | BLOCKED | NEEDS_CONTEXT), commit hash(es) + branch, one-line test summary, concerns (≤ 5 lines).
