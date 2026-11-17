#!/usr/bin/env python3
"""limit-wait r3: mutants (each on a copy of the tree, one incremental target per unit).
usage (cwd = synced tree): python3 tools/limit_wait_r3_mut.py <outdir> <unit> name...   (no names = all)
A mutant is KILLED when its test subset fails; BASE (unmutated) must pass both subsets."""
import os
import shutil
import subprocess
import sys

C, O, K, P = "engine/src/topo/core.rs", "engine/src/topo/optimise.rs", "engine/src/control.rs", "plugin/topor"
chk = lambda n: (C, f'self.ctl.test_point("loop stage {n} check");\n                if stop() {{', f'self.ctl.test_point("loop stage {n} check");\n                if false && stop() {{')
M = {
    # round 2 (author's 7, still standing)
    "scope": (C, "crate::control::in_loop_scope(|| optimise(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock))", "optimise(&mut sp, &mut arch, &params, &mut stop, &mut inner, &clock)"),
    "chk1": chk(1), "chk2": chk(2), "chk3": chk(3),
    "due": (C, "reached && (wait_s > 0.0 || cycle_dropped)", "reached && wait_s > 0.0"),
    "loopmsg": (C, 'let in_loop = soft == Some(crate::control::SoftStep::Loop) || w.step.contains("TopoR loop");', "let in_loop = true;"),
    "latch": (O, "if hit || inner() {", "if inner() {"),
    # the r2 verifier's survivors
    "v_within": (C, "Some(g) if w.wait_s <= g =>", "Some(g) if true || w.wait_s <= g =>"),
    "v_last": (C, "} else { (runtime_s - limit_s).max(0.0) };", "} else { 0.0 };"),
    "v_idle": (K, 'self.limit.soft_enter(self.run, 0, ""); // computes nothing', 'let _ = 0; // computes nothing'),
    "v_cancel": (C, "if ctl.limit.passed() && rep.dropped && !stop_now() {", "if ctl.limit.passed() && rep.dropped {"),
    "p_run": (f"{P}/run.py", 'if reached and (tl["wait_s"] > 0 or tl["cycle_dropped"] or tl["draw_grace_hit"]):', "if False:"),
    "p_postrun": (f"{P}/postrun.py", 'lw = lines("time limit wait: ")', "lw = []"),
    # round 3: the soft step
    "s_change": (K, "if cur != next && began_before {", "if false && began_before {"),
    "s_every": (K, "if cur != next && began_before {", "if began_before {"),
    "s_before": (K, "if cur != next && began_before {", "if cur != next {"),
    "s_inherit": (K, "if cur == SoftSlot::Never && i > 0 {", "if false {"),
    "s_cycle": (K, "Some(_) if whole_cycle => 1,", "Some(_) if false => 1,"),
    "s_pick": (C, 'ctl.stage(rnd, rnds, "picking the variant to draw");', "let _ = rnd;"),
    "s_cutwait": (K, "self.cut_in_wait.store(true, Ordering::Relaxed);", "let _ = 0;"),
    "s_post": (C, """    post(&mut best);
    best.runtime_s = t0.elapsed().as_secs_f64();""", """    best.runtime_s = t0.elapsed().as_secs_f64();
    post(&mut best);"""),
    # round 3: drawing_grace_s
    "d_reduce": (C, "ctl.is_cancelled() || limit_cut() || draw_grace_over());", "ctl.is_cancelled() || limit_cut());"),
    "d_start": (K, "Some(d.max(draw_start) + Duration::from_secs_f64(g))", "Some(d + Duration::from_secs_f64(g))"),
    "d_erbs": (C, "let erbs_deadline = if erbs_soft { ctl.limit.draw_bound(t_draw) } else", "let erbs_deadline = if erbs_soft { ctl.deadline().map(|d| d.max(floor)) } else"),
    "d_text": (C, "let red_grace_cut = rs.timed_out && !red_limit_cut && grace_stop.get() && !ctl.is_cancelled();", "let red_grace_cut = false;"),
    "d_hard": (C, 'ctl.limit.set_draw_grace(if opts.timeout_mode == "hard" { -1.0 } else { opts.drawing_grace_s });', "ctl.limit.set_draw_grace(opts.drawing_grace_s);"),
    # round 3: Python
    "p_none": (f"{P}/report.py", "    except (TypeError, ValueError):\n        return f\"run.{key} unbounded\"", "    except (TypeError, ValueError):\n        raise"),
    "p_cause": (f"{P}/report.py", '    if tl.get("grace_hit"):\n        return f"{grace_text', '    if tl.get("cycle_dropped"):\n        return f"{grace_text'),
}
R, unit = sys.argv[1], sys.argv[2]
names = sys.argv[3:] or list(M)
src = os.getcwd()
W = f"/root/lw3-mut-{unit}"
shutil.rmtree(W, ignore_errors=True)
subprocess.run(f"mkdir -p {W} && tar --exclude=./target --exclude=./scratch --exclude=./.git -cf - . | tar -xf - -C {W}", shell=True, check=True)
if os.path.isdir(os.path.join(src, "scratch")):
    os.symlink(os.path.join(src, "scratch"), os.path.join(W, "scratch"))
env = {**os.environ, "CARGO_TARGET_DIR": f"/root/lw3-mut-target-{unit}", "RUST_TEST_THREADS": "4"}
rust_cmd = ("cargo test --release --no-fail-fast --manifest-path engine/Cargo.toml --lib --test topo_limit_wait "
            "--test topo_drawing_grace")
py_cmd = ("PYTHONPATH=plugin python -m pytest -q -p no:cacheprovider tests/plugin/test_report.py tests/plugin/test_run_time_limit.py "
          "tests/plugin/test_postrun.py tests/plugin/test_messages.py tests/plugin/test_config.py -x -q")
for n in ["BASE"] + names:
    for f in {m[0] for m in M.values()}:
        shutil.copy(os.path.join(src, f), os.path.join(W, f))
    f = None
    if n != "BASE":
        f, a, b = M[n]
        p = os.path.join(W, f)
        s = open(p).read()
        if s.count(a) != 1:
            print(f"MUTANT {n}: pattern count {s.count(a)} (edit failed)", flush=True)
            continue
        open(p, "w").write(s.replace(a, b, 1))
    if n == "BASE":
        cmd = rust_cmd + " ; r1=$? ; " + py_cmd + " ; exit $(( r1 | $? ))"
    else:
        cmd = py_cmd if f.endswith(".py") else rust_cmd
    log = os.path.join(R, f"mut-{unit}-{n}.log")
    rc = subprocess.run(cmd, shell=True, cwd=W, env=env, stdout=open(log, "w"), stderr=subprocess.STDOUT).returncode
    fails = [ln.strip() for ln in open(log) if ("FAILED" in ln and ln.startswith("test ")) or ln.startswith("FAILED ") or "error[" in ln][:6]
    print(f"MUTANT {n}: {'KILLED' if rc else 'SURVIVED'} rc={rc} {fails}", flush=True)
