#!/usr/bin/env python3
"""stable-label r4: mutants of the lines this round changed (each on a copy of the tree, one incremental target per
unit). usage (cwd = synced tree): python3 tools/sl4_mut.py <outdir> <rust|py> name...  (no names = all of the kind)
KILLED = its test subset fails; BASE (unmutated) must pass."""
import os
import shutil
import subprocess
import sys

C, P = "engine/src/topo/core.rs", "plugin/topor"
M = {
    # F1: the drawn run's cut decides stable vs timeout
    "r_okkept": (C, "    let ok_kept = ok_stop && !draw_cut;\n", "    let ok_kept = ok_stop;\n"),
    "r_timed_old": (C, "best.timed_out = (time_hit || draw_cut) && !(cancel && ok_kept);", "best.timed_out = time_hit && !(cancel && ok_stop);"),
    "r_stop_nodraw": (C, "    } else if time_hit || draw_cut {\n        \"timeout\"", "    } else if time_hit {\n        \"timeout\""),
    "r_stitch_note": (C, "self.draw_cut = interrupted || cycle_stitch_cut;", "self.draw_cut = interrupted;"),
    "r_stitch_warn": (C, "if ok_at_stable() && !this_cut {", "if ok_at_stable() {"),
    "r_why": (C, "let why = if draw_cut && (ok_stop || drawn_stable && !cancel) {", "let why = if draw_cut && ok_stop {"),
    "r_why_ok": (C, "if ok_stop { \"OK came at Routes stable\" } else { \"the routes were stable\" }", "\"the routes were stable\""),
    # F2: the press is judged by the label it was pressed on
    "p_latch": (f"{P}/ui_wx.py", "        self.ok_at_press = self.stable  # what", "        self.ok_at_press = None  # what"),
    "p_pump_first": (f"{P}/ui_wx.py", "        self._pump()\n        dlg.update(p)\n        self._pump()\n", "        dlg.update(p)\n        self._pump()\n"),
    "p_runpy": (f"{P}/run.py", "ok = bool(p.stable) if pressed is None else bool(pressed)", "ok = bool(p.stable)"),
    "p_label": (f"{P}/ui_wx.py", "ok = self.stable if self.ok_at_press is None else self.ok_at_press", "ok = self.stable"),
    # F3: the progress dialog's limit / clock / caps follow "Use the current default"
    "p_eff": (f"{P}/ui_wx.py", "return defaults.get(sec, key) if f\"{sec}.{key}\" in use else cfg.get(sec, key)", "return cfg.get(sec, key)"),
    "p_eff_over": (f"{P}/ui_wx.py", "if sec == \"run\" and key in d.overrides:", "if False:"),
    # F4: no ".:" in the cause pane
    "p_cause": (f"{P}/ui_wx.py", "if txt.endswith(\".\") and not txt.endswith(\"..\"):", "if False:"),
    "p_cause_ell": (f"{P}/ui_wx.py", "if txt.endswith(\".\") and not txt.endswith(\"..\"):", "if txt.endswith(\".\"):"),
}
R, kind = sys.argv[1], sys.argv[2]
names = sys.argv[3:] or [n for n in M if n.startswith("r_" if kind == "rust" else "p_")]
src = os.getcwd()
W = f"/root/sl4-mut-{kind}"
shutil.rmtree(W, ignore_errors=True)
subprocess.run(f"mkdir -p {W} && tar --exclude=./target --exclude=./scratch --exclude=./.git -cf - . | tar -xf - -C {W}", shell=True, check=True)
if os.path.isdir(os.path.join(src, "scratch")):
    os.symlink(os.path.join(src, "scratch"), os.path.join(W, "scratch"))
env = {**os.environ, "CARGO_TARGET_DIR": f"/root/sl4-mut-target-{kind}", "RUST_TEST_THREADS": "4"}
rust_cmd = "cargo test --release --no-fail-fast --manifest-path engine/Cargo.toml --test topo_stop_reason"
py_cmd = ("PYTHONPATH=plugin python -m pytest -q -p no:cacheprovider tests/plugin/test_ui_wx.py "
          "tests/plugin/test_run_flow.py -k 'press or use_the_current_default or cause_pane or button_read' -rs")
for n in ["BASE"] + names:
    for f in {m[0] for m in M.values()}:
        shutil.copy(os.path.join(src, f), os.path.join(W, f))
    if n != "BASE":
        f, a, b = M[n]
        p = os.path.join(W, f)
        s = open(p).read()
        if s.count(a) != 1:
            print(f"MUTANT {n}: pattern count {s.count(a)} (edit failed)", flush=True)
            continue
        open(p, "w").write(s.replace(a, b, 1))
    cmd = rust_cmd if kind == "rust" else py_cmd
    log = os.path.join(R, f"mut-{kind}-{n}.log")
    rc = subprocess.run(cmd, shell=True, cwd=W, env=env, stdout=open(log, "w"), stderr=subprocess.STDOUT).returncode
    text = open(log).read()
    fails = [ln.strip() for ln in text.splitlines() if ("FAILED" in ln and ln.startswith("test ")) or ln.startswith("FAILED ") or "error[" in ln][:6]
    bad = [w for w in ("Gtk-ERROR", "core dumped") if w in text]
    if kind == "py" and " passed" not in text and " failed" not in text:
        bad.append("NOTHING RAN")
    verdict = "INVALID" if bad else ("KILLED" if rc else "SURVIVED")
    print(f"MUTANT {n}: {verdict} rc={rc} {fails} {bad or ''}", flush=True)
