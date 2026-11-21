"""first-draw-45 r6 (TEMP): mutants of the P4 lines, each in a copy of the tree with its own target.
    MUTS=a,b python3 tools/fd45r6_mutants.py <tree> <out_dir> <work_tree> <target>
A mutant is killed when the U6/K5 tests of tests/topo_first_draw_45.rs fail with it.
"""
import os, shutil, subprocess, sys, time
O = "engine/src/topo/octi.rs"
C = "engine/src/topo/core.rs"
M = [
    ("P4-chord", O, 'if !fd45_off("p4") {', "if false {"),
    ("P4-straighten", C, 'if octi_on && opts.octi_straighten && !super::octi::fd45_off("p4s") {', "if false {"),
    ("S-follow", O, "plen(mid) + style_price(&cfg, pitch, &ext) - cfg.pull * follow(mid)", "plen(mid) + style_price(&cfg, pitch, &ext)"),
    ("S-zforms", O, "for &(c, _, kk, need) in &side {", "for &(c, _, kk, need) in side.iter().take(0) {"),
    ("S-passes", C, "const STRAIGHTEN_PASSES: usize = 4;", "const STRAIGHTEN_PASSES: usize = 1;"),
    ("S-endleg", O, "octi_dir(q) != octi_dir(o) || q.len() + 1_000.0 < o.len().min(leff)", "false"),
]
def main(tree, out, work, tgt):
    os.makedirs(out, exist_ok=True); rows = []
    for mid, f, old, new in [x for x in M if x[0] in os.environ.get("MUTS", x[0]).split(",")]:
        if os.path.exists(work): shutil.rmtree(work)
        shutil.copytree(tree, work, symlinks=True, ignore=shutil.ignore_patterns("target", ".git", "scratch"))
        p = os.path.join(work, f); s = open(p).read()
        if s.count(old) < 1:
            rows.append((mid, f, "no-match", 0)); continue
        line = s[: s.index(old)].count("\n") + 1
        open(p, "w").write(s.replace(old, new))
        for root, _, fs in os.walk(os.path.join(work, "engine/src")):
            for x in fs: os.utime(os.path.join(root, x))
        t0 = time.time()
        r = subprocess.run(["cargo", "test", "--release", "--test", "topo_first_draw_45", "--", "u6", "k5"], cwd=os.path.join(work, "engine"), env={**os.environ, "CARGO_TARGET_DIR": tgt}, capture_output=True, text=True)
        log = r.stdout + r.stderr
        open(os.path.join(out, f"mut-{mid}.log"), "w").write(log[-6000:])
        v = "build-fail" if ("error[" in log or "could not compile" in log) else ("killed" if r.returncode != 0 else "survived")
        rows.append((mid, f"{f}:{line}", v, round(time.time() - t0)))
        open(os.path.join(out, "mutants.tsv"), "w").write("\n".join("\t".join(map(str, x)) for x in rows) + "\n")
main(*sys.argv[1:])
