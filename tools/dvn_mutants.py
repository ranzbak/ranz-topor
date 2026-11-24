"""draw-via-neck mutants (A via shift, B neck trim, C pad tip anchor): each mutant is one text replacement in a copy of
engine/; the targeted release tests run against it; killed = a test fails. Run on the VM (one tree, one target dir per
call, mutants one after another so the build is incremental).

usage: python3 tools/dvn_mutants.py OUT_DIR GROUP     (GROUP: a | bc | all; results OUT_DIR/mutants-GROUP.json)"""
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TESTS = ["topo_dvn", "topo_first_draw_45", "topo_vias54"]
D, P = "src/topo/draw.rs", "src/topo/padend.rs"
MUTANTS = {
    "a": [
        ("A1 side test: own triangle only", D,
         "let tris: Vec<super::state::TriId> = if super::state::point_in_poly(p, &pr.tris[t].map(|v| pr.pos[v])) {",
         "let tris: Vec<super::state::TriId> = if true {"),
        ("A2 first-traversal break kept", D,
         "if best.is_some_and(|b| b.1 != k) && (self.opts.via_shift_mm <= 0.0 || best_jam <= 1.0) {",
         "if best.is_some_and(|b| b.1 != k) {"),
        ("A3 no shift distance price", D,
         "price + 0.5 * self.octi.room * dv * (xs_ + xs_ * xs_)", "price"),
        ("A4 no shifted sites", D,
         "sites.extend(self.via_shift_sites(w, plan, i, ka, kb.max(ka), out.last().map(|l| l.0), need_r, sz.d as f64));",
         "let _ = need_r;"),
    ],
    "bc": [
        ("B1 fit_neck off", D,
         "        let wi = &self.st.wires[w];\n        let ext = |c: Vec2|",
         "        if wn >= 0.0 { return *neck; }\n        let wi = &self.st.wires[w];\n        let ext = |c: Vec2|"),
        ("B2 no room growth (trim only)", D,
         "if room <= 0.0 || r1 <= r0 + 1.0 {", "if room <= 0.0 || r1 <= r0 + 1.0 || room > 0.0 {"),
        ("C1 anchor off", P,
         "        let mut n_done = 0;\n        let wires: Vec<WireId> = self.erbs.keys().copied().collect();",
         "        let mut n_done = 0;\n        let wires: Vec<WireId> = Vec::new();"),
        ("C2 band guard off", P,
         "            if let Some(why) = self.band_in_way(w, layer, &ring, &[c, tp, q[j]], &q[..=j]) {",
         "            if let Some(why) = None::<WireId> {"),
        ("C3 band guard: no pitch clause", P,
         "if inside || cross || dn < need.min(dmin(p, old)) - 1.0 {", "if inside || cross || (dn < 0.0 && need > dmin(p, old)) {"),
        ("C4 side entries only (no off-axis trigger)", P,
         "        if !side && on_axis {", "        if !side {"),
    ],
}


def main():
    out = Path(sys.argv[1])
    group = sys.argv[2]
    out.mkdir(parents=True, exist_ok=True)
    tree = out / f"mut-tree-{group}"
    target = out / f"mut-target-{group}"
    if tree.exists():
        shutil.rmtree(tree)
    shutil.copytree(ROOT / "engine", tree, ignore=shutil.ignore_patterns("target"))
    env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS=os.environ.get("RAYON_NUM_THREADS", "4"), RUST_TEST_THREADS="4")
    muts = MUTANTS["a"] + MUTANTS["bc"] if group == "all" else MUTANTS[group]
    res = []
    for name, f, old, new in muts:
        src = (ROOT / "engine" / f).read_text()
        if src.count(old) != 1:
            res.append({"mutant": name, "result": f"NOT APPLIED ({src.count(old)} matches)"})
            continue
        for g in ("src/topo/draw.rs", "src/topo/padend.rs"):
            shutil.copy(ROOT / "engine" / g, tree / g)
        (tree / f).write_text(src.replace(old, new))
        for g in ("src/topo/draw.rs", "src/topo/padend.rs"):
            os.utime(tree / g)
        t0 = time.time()
        cmd = ["cargo", "test", "--release", "--no-fail-fast", "--manifest-path", str(tree / "Cargo.toml")]
        for t in TESTS:
            cmd += ["--test", t]
        p = subprocess.run(cmd, env=env, capture_output=True, text=True, pass_fds=())
        log = p.stdout + p.stderr
        failed = sorted({ln.split()[1] for ln in log.splitlines() if ln.startswith("test ") and ln.rstrip().endswith("FAILED")})
        built = "test result:" in log
        verdict = "BUILD FAILED" if not built else ("killed" if failed else "survived")
        res.append({"mutant": name, "result": verdict, "failed": failed, "s": round(time.time() - t0)})
        (out / f"mut-{name.split()[0]}.log").write_text(log[-20000:])
        print(name, verdict, failed, flush=True)
    (out / f"mutants-{group}.json").write_text(json.dumps(res, indent=1))


if __name__ == "__main__":
    main()
