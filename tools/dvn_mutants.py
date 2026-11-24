"""draw-via-neck mutants (A via shift, B neck trim, C pad tip anchor): each mutant is one text replacement in a copy of
engine/; the targeted release tests run against it; killed = a test fails. Run on the VM (one tree, one target dir per
call, mutants one after another so the build is incremental).

usage: python3 tools/dvn_mutants.py OUT_DIR GROUP     (GROUP: a | bc | c3 | r4 | all; results OUT_DIR/mutants-GROUP.json)"""
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
    ],
    # r4 (lead 2026-11-24): A's sites outside the interval are gone (A4 with them); the ring inside it stays; B's
    # no-legal-width fallback takes the widest width that adds no violation.
    "r4": [
        ("A5 r4: no ring sites in the interval", D,
         "sites.extend(self.via_shift_sites(w, ka, kb.max(ka), sz.d as f64));", "let _ = sz;"),
        ("A6 r4: no price for other wires' planned via sites", D,
         "self.via_plan_price_near(w, ka, kb.max(ka), k, p, dr) + self.via_site_price(w, p, dr) }", "self.via_plan_price_near(w, ka, kb.max(ka), k, p, dr) }"),
        ("A8 r4: plan price over the interval's triangles only", D,
         "self.via_plan_price_near(w, ka, kb.max(ka), k, p, dr) + self.via_site_price", "self.via_plan_price(w, ka, kb.max(ka), p, dr) + self.via_site_price"),
        ("A7 r4: no via_shift_mm bound on a later traversal's site", D,
         "                                if sh > self.opts.via_shift_mm * 1e6 {\n", "                                if false {\n"),
        ("B3 r4: no legal width -> the floor", D,
         "let wn = if self.opts.neck_trim { self.plan_neck_widest(w, lid, &pts, pn) } else { pn.min_w };", "let wn = pn.min_w;"),
        ("B4 r4: widest without the no-new-violation test", D,
         "let ok = |wn: f64| hit(wn).is_subset(&floor);", "let ok = |wn: f64| { let _ = (hit(wn), &floor); true };"),
        ("B5 r4: forced runs keep the narrowest width", D,
         "let wn = if wmin <= pn.min_w + 0.5 { self.plan_neck_widest(w, lid, &pts, pn) } else { wmin };", "let wn = wmin;"),
    ],
    "bc": [
        ("B1 fit_neck off", D,
         "        let wi = &self.st.wires[w];\n        let ext = |c: Vec2|",
         "        if wn >= 0.0 { return *neck; }\n        let wi = &self.st.wires[w];\n        let ext = |c: Vec2|"),
        ("B2 no room growth (trim only)", D,
         "if room <= 0.0 || r1 <= r0 + 1.0 {", "if room <= 0.0 || r1 <= r0 + 1.0 || room > 0.0 {"),
        ("C1 anchor off (r3: at draw time)", D,
         "Some(b) if self.opts.pad_tip_anchor && b.len() >= 2 && (r == 0 || r == plan.layers.len() - 1) => {",
         "Some(b) if false && self.opts.pad_tip_anchor && b.len() >= 2 && (r == 0 || r == plan.layers.len() - 1) => {"),
        ("C2 band guard off", P,
         "            if let Some(why) = self.band_in_way(w, layer, &ring, &[c, tp, q[j]], &q[..=j]) {",
         "            if let Some(why) = None::<WireId> {"),
        ("C3 band guard: no pitch clause", P,
         "if inside || cross || dn < need.min(dmin(p, old)) - 1.0 {", "if inside || cross || (dn < 0.0 && need > dmin(p, old)) {"),
        ("C4 side entries only (no off-axis trigger)", P,
         "        if !side && on_axis {", "        if !side {"),
        ("C5 r3: no drawn-copper ring test", P,
         "if self.foreign_inside(wi.net, layer, &ring) || self.drawn_inside(wi.net, layer, &ring) {",
         "if self.foreign_inside(wi.net, layer, &ring) {"),
        ("C6 r3: band guard reads every band (drawn and reserved too)", P,
         "for rb in runs.iter().filter(|r| r.layer == layer && r.path.len() >= 2 && !reserved(r)) {",
         "for rb in runs.iter().filter(|r| r.layer == layer && r.path.len() >= 2 && (true || !reserved(r))) {"),
        ("C7 r3: the old tail's reservation kept (adopted at attempt -1)", D,
         "                        for h in res.take().map(|x| x.handles).into_iter().flatten() {\n                            self.uncommit(h);\n                        }\n                        self.stats.tip_anchored += 1;",
         "                        self.stats.tip_anchored += 1;"),
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
    muts = MUTANTS["a"] + MUTANTS["r4"] + MUTANTS["bc"] if group == "all" else [m for m in MUTANTS["bc"] if m[0].startswith(("C1", "C5", "C6", "C7"))] if group == "c3" else MUTANTS[group]
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
