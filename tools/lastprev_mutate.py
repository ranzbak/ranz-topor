#!/usr/bin/env python3
"""last-preview mutants of the lines this branch changed, each killed (or not) by the branch's targeted tests
(engine/tests/last_preview.rs + control unit test for the engine, tests/plugin/test_last_preview.py +
test_ui_wx.py for the plugin).

    tools/lastprev_mutate.py list
    tools/lastprev_mutate.py run <out_dir>     each mutant applied, its tests run, restored; one line per mutant
    LASTPREV_MUT_ONLY=a,b ...                  only these mutants

A mutant = (file, old text, new text, kind); `old` must occur exactly once (else 'stale'). Run with CARGO_TARGET_DIR
set (the engine is rebuilt per Rust mutant) and the venv's python for the plugin ones."""
import os, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
C = "engine/src/control.rs"
K = "engine/src/topo/core.rs"
U = "plugin/topor/ui_wx.py"
R = "plugin/topor/run.py"
S = "plugin/topor/schema.py"
M = {
    # engine
    "want_never_cleared": (C, "                self.sketches.want.store(false, Ordering::Relaxed);\n", "", "rs"),
    "best_slot_run0": (C, "            let k = (sk.run as usize).saturating_sub(1);", "            let k = 0usize;", "rs"),
    "cycles_no_inc": (K, "        p.cycles_total += 1;", "        p.cycles_total += 0;", "rs"),
    "cycles_count_zero": (K, "    if cycle > 0 {\n        p.cycles_total += 1;", "    if cycle < usize::MAX {\n        p.cycles_total += 1;", "rs"),
    "pick_sketch_own_always": (K, "                    let sk = if own == pick {", "                    let sk = if true {", "rs"),
    "keep_items_worse": (K, "ji.as_ref().is_none_or(|(b, _)| obj.better(c, b))", "ji.as_ref().is_none_or(|(b, _)| obj.better(b, c))", "rs"),
    "run_pick_no_cycle": (K, "    p.pick_cycles[k] = cycle as u32;", "    p.pick_cycles[k] = 0;", "rs"),
    # plugin
    "total_not_monotone": (U, "            self._total_seen = max(self._total_seen, int(getattr(p, \"cycles_total\", 0) or 0))",
                           "            self._total_seen = int(getattr(p, \"cycles_total\", 0) or 0) if p.loop_cycle > 1 else 1", "py"),
    "predicted_never": (U, "    return not getattr(p, \"last_round\", 0) or getattr(p, \"loop_at_s\", 0.0) > getattr(p, \"last_at_s\", 0.0)",
                        "    return False", "py"),
    "restart_hidden": (U, "restart=W[\"round_context_restart\"].format(n=rs) if rs else \"\"", "restart=\"\"", "py"),
    "poll_ignores_seq": (R, "    if p.sketch_seq == sk[\"seq\"] and kind == sk[\"kind\"]:", "    if kind == sk[\"kind\"] and sk[\"seq\"] != -1:", "py"),
    "default_best": (S, "    _s(\"run\", \"progress_preview\", \"str\", \"last\",", "    _s(\"run\", \"progress_preview\", \"str\", \"best\",", "py"),
}
RS_TEST = ["cargo", "test", "--release", "--test", "last_preview", "--lib", "--", "sketch_request_publish_and_slots",
           "copper_is_the_same", "plan_sketches", "the_cycle_count"]
PY_TEST = [sys.executable, "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider", "tests/plugin/test_last_preview.py",
           "tests/plugin/test_ui_wx.py"]


def run(out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    only = [x for x in os.environ.get("LASTPREV_MUT_ONLY", "").split(",") if x]
    lines = []
    for name, (f, old, new, kind) in M.items():
        if only and name not in only:
            continue
        p = ROOT / f
        text = p.read_text()
        if text.count(old) != 1:
            lines.append(f"{name} stale")
            print(lines[-1], flush=True)
            continue
        p.write_text(text.replace(old, new))
        try:
            if kind == "rs":
                cmd, cwd = RS_TEST, ROOT / "engine"
            else:
                cmd, cwd = PY_TEST, ROOT
            env = {**os.environ, "PYTHONPATH": f"{ROOT}/plugin:{ROOT}:" + os.environ.get("PYTHONPATH", "")}
            r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, timeout=1800)
            (out / f"{name}.log").write_text(r.stdout + r.stderr)
            built = not (kind == "rs" and "error[" in r.stderr)
            lines.append(f"{name} {'killed' if r.returncode != 0 and built else 'NOT BUILT' if not built else 'SURVIVED'}")
        finally:
            p.write_text(text)
        print(lines[-1], flush=True)
    (out / "mutants.txt").write_text("\n".join(lines) + "\n")
    return 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["list"]:
        print("\n".join(M))
    else:
        sys.exit(run(Path(sys.argv[2])))
