#!/usr/bin/env python3
"""limit-wait r6 mutants (the lines this round changed): apply one mutant, run the targeted tests, restore.
usage: tools/lw6_mut.py [name ...]   (run from the worktree root; through slot.sh; env CARGO_TARGET_DIR set)"""
import subprocess, sys, pathlib
E, C, K, R = "engine/src/topo/erbs.rs", "engine/src/topo/core.rs", "engine/src/control.rs", "plugin/topor/report.py"
M = {
    "e_cancel": (E, "if self.stop.as_ref().is_some_and(", "if false && self.stop.as_ref().is_some_and("),
    "e_deadline": (E, "if self.deadline.is_some_and(|d| now >= d) {", "if false {"),
    "e_hardened_cap": (E, "let cap = l.hardened() || wall_cap", "let cap = wall_cap"),
    "e_floor": (E, "self.floor.map_or(c, |f| c.max(f))", "c"),
    "e_ge": (E, "wall_cap.is_some_and(|c| b <= c) => Some(ErbsStop::Bound)", "wall_cap.is_some_and(|c| b >= c) => Some(ErbsStop::Bound)"),
    "e_nocap_cmp": (E, "(Some(b), true) if l.hardened() || wall_cap.is_some_and(|c| b <= c) =>", "(Some(_b), true) if true =>"),
    "e_hard_guard": (E, "(Some(b), true) if l.hardened() || wall_cap", "(Some(b), true) if wall_cap"),
    "e_nonow": (E, ".filter(|&b| now >= b);", ";"),
    "e_first_layer": (E, "out.stopped.get_or_insert(why);", "out.stopped = Some(why);"),
    "e_first_realise": (E, "        if out.stopped.is_none() {\n            out.stopped = le.stopped;", "        if le.stopped.is_some() {\n            out.stopped = le.stopped;"),
    "c_draw_start": (C, "draw_start: erbs_soft.then_some(t_draw),", "draw_start: None,"),
    "c_stop": (C, "stop: (!draw_through.get()).then(|| ctl.cancel.clone()),\n            limit: Some", "stop: Some(ctl.cancel.clone()),\n            limit: Some"),
    "c_floor": (C, "floor: Some(floor),\n            fail_first", "floor: None,\n            fail_first"),
    "c_note": (C, "if r.timed_out > 0 && r.stopped == Some(ErbsStop::Bound) {", "if r.timed_out > 0 {"),
    "c_by_hard": (C, "_ if ctl.limit.hard_mode() =>", "_ if false =>"),
    "c_by_cancel": (C, 'Some(ErbsStop::Cancel) => "the run was cancelled",', 'Some(ErbsStop::Cancel) => "[run] drawing_grace_s passed past the time limit",'),
    "c_deadline_soft": (C, "deadline: if erbs_soft { None } else", "deadline: if erbs_soft { ctl.limit.draw_bound(t_draw) } else"),
    "c_limit_none": (C, "limit: Some(ctl.limit.clone()),\n            draw_start", "limit: None,\n            draw_start"),
    "w_draw_cut": (C, "draw_cut: steps.contains(&CutStep::Drawing),", "draw_cut: false,"),
    "w_draw_note": (C, 'if w.draw_cut { "" } else { "every wire was drawn; " },', '"every wire was drawn; ",'),
    "k_hardened": (K, "        self.hardened.store(true, Ordering::Relaxed);\n", ""),
    "p_traces_cut": (R, 'traces_cut = "drawing" in steps if', 'traces_cut = False and "drawing" in steps if'),
    "p_cut_fallback": (R, 'else tl.get("cut") == "drawing"', 'else False'),
}
RUST = ["cargo", "test", "--release", "--manifest-path", "engine/Cargo.toml", "--no-fail-fast", "--test", "topo_limit_erbs_end", "--test", "vf_lw4",
        "--test", "topo_drawing_grace", "--test", "cancel_latency", "--lib", "--", "wait_line", "erbs", "radii", "control::", "lw_"]
PY = ["/home/paul/work/kicad/plugins/topor/.venv/bin/python", "-m", "pytest", "-q", "-p", "no:cacheprovider", "tests/plugin/test_report.py", "tests/plugin/test_postrun.py"]
names = sys.argv[1:] or list(M)
for n in names:
    f, a, b = M[n]
    p = pathlib.Path(f)
    src = p.read_text()
    if src.count(a) != 1:
        print(f"MUT {n}: INVALID (pattern x{src.count(a)})", flush=True)
        continue
    p.write_text(src.replace(a, b))
    try:
        r = subprocess.run(PY if f.endswith(".py") else RUST, capture_output=True, text=True, timeout=900)
        out = r.stdout + r.stderr
        if "error[E" in out or "could not compile" in out:
            print(f"MUT {n}: INVALID (compile)", flush=True)
            continue
        failed = sorted({l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.rstrip().endswith("FAILED")} |
                        {l.split("::")[-1].split()[0] for l in out.splitlines() if l.startswith("FAILED ")})
        print(f"MUT {n}: {'KILLED' if r.returncode else 'SURVIVED'} rc={r.returncode} by {failed[:8]}", flush=True)
    finally:
        p.write_text(src)
