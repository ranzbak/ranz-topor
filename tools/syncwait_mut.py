#!/usr/bin/env python3
"""sync-wait mutants (DoD 1): each mutant changes one line of this branch; the named test must fail.

    python3 tools/syncwait_mut.py <tree-copy> <out.tsv> [<first mutant index>]

Runs in a COPY of the tree (own target dir: <tree-copy>/engine/target). Per mutant: apply, run its test command,
restore; killed = the test command failed, or hung past 600 s (a wait that never ends). O(mutants x (build + test)).
"""
import subprocess
import sys
from pathlib import Path

T = "engine/src/topo/table.rs"
C = "engine/src/topo/core.rs"
K = "engine/src/control.rs"
lib = lambda f: ["cargo", "test", "--release", "--lib", "--", f]  # noqa: E731
MUTANTS = [
    ("commit past min done - lag", T, "h.saturating_sub(self.lag) };", "h };", lib("topo::table")),
    ("view without the replay", T, "let st = if cycles.is_empty() {", "let st = if true {", lib("topo::table")),
    ("lag off by one", T, "let need = cycle.saturating_sub(1).saturating_sub(self.lag);", "let need = cycle.saturating_sub(1).saturating_sub(self.lag + 1);", lib("topo::table")),
    ("lag ignored by the view", T, "let need = cycle.saturating_sub(1).saturating_sub(self.lag);", "let need = cycle.saturating_sub(1);", lib("topo::table")),
    ("ready ignores ended runs", T, "(0..g.done.len()).all(|r| r == run || g.ended[r] || g.done[r].is_some_and(|c| c >= need))", "(0..g.done.len()).all(|r| r == run || g.done[r].is_some_and(|c| c >= need))", lib("topo::")),
    ("any waiter takes a free slot", C, "while !(g.free > 0 && g.queue.first() == Some(&key)) {", "while !(g.free > 0) {", lib("topo::core::tests::a_")),
    ("turn only to first-slot waiters", C, "let lower = g.queue.first().is_some_and(|k| (k.0, k.1, k.2) < (1, clock, run));", "let lower = g.queue.first().is_some_and(|k| k.0 == 0);", lib("topo::core::tests::a_run_that_gave")),
    ("turn to any waiter", C, "let lower = g.queue.first().is_some_and(|k| (k.0, k.1, k.2) < (1, clock, run));", "let lower = !g.queue.is_empty();", lib("topo::core::tests::a_run_that_gave")),
    ("idle not counted", C, "self.idle_s += self.free.max(0) as f64 * dt;", "self.idle_s += 0.0 * dt;", lib("topo::core::tests::slot_idle")),
    ("idle counted outside the loop", C, "        if self.loops > 0 {\n            let dt", "        if true {\n            let dt", lib("topo::core::tests::slot_idle")),
    ("cycle ring not capped", K, "if self.cycle_lines.len() >= CYCLE_LINES {", "if false {", lib("control::tests::the_cycle_lines")),
    ("cycle lines not counted", K, "self.cycles_logged += 1;", "self.cycles_logged += 0;", lib("control::tests::the_cycle_lines")),
    ("python: a line twice", "plugin/topor/run.py", "out += [ln for i, ln in enumerate(lines) if first + i > seen]", "out += [ln for i, ln in enumerate(lines) if first + i >= seen]",
     ["python", "-m", "pytest", "-q", "-x", "tests/plugin/test_score_progress.py", "-k", "cycle_log_lines"]),
    ("python: gap not said", "plugin/topor/run.py", 'out = [f"loop cycle log: {first - seen - 1} cycle lines missed"] if first > seen + 1 else []', "out = []",
     ["python", "-m", "pytest", "-q", "-x", "tests/plugin/test_score_progress.py", "-k", "cycle_log_lines"]),
]


def main(tree: str, out: str, first: int = 0):
    root = Path(tree)
    rows = []
    for name, f, old, new, cmd in MUTANTS[first:]:
        p = root / f
        src = p.read_text()
        if src.count(old) != 1:
            rows.append((name, "NOT APPLIED", f"{src.count(old)} matches"))
            continue
        p.write_text(src.replace(old, new))
        try:
            cwd = root / "engine" if cmd[0] == "cargo" else root
            try:
                r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=600)
                tail = (r.stdout + r.stderr).strip().splitlines()[-1:] or [""]
                rows.append((name, "killed" if r.returncode != 0 else "SURVIVED", tail[0][:160]))
            except subprocess.TimeoutExpired:
                rows.append((name, "killed (hang)", "test still running after 600 s"))
        finally:
            p.write_text(src)
        print("\t".join(rows[-1]), flush=True)
    Path(out).write_text("".join("\t".join(r) + "\n" for r in rows))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 0)
