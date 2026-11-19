#!/usr/bin/env python3
"""tw4-r2 mutants of the lines this round changed ((2) via_best_point / via_over_ramp, (3) pour_cuts), each killed (or
not) by the targeted engine tests.

    tools/tw4r2_mutate.py list
    tools/tw4r2_mutate.py run <out_dir>     (VM: each mutant applied, tests run, restored; one line per mutant)
    TW4R2_MUT_ONLY=a,b ...                  (only these mutants)

A mutant = (file, old text, new text); `old` must occur exactly once (else 'stale')."""
import os, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V = "engine/src/topo/viacut.rs"
F = "engine/src/topo/refine.rs"
P = "engine/src/topo/pourplan.rs"
O = "engine/src/topo/optimise.rs"
P2 = "engine/src/topo/pack.rs"
M = {
    "ramp_step": (V, "    Some(((ov / ramp).min(1.0) * f, ov >= ramp))", "    Some((f, ov >= ramp))"),
    "ramp_count_always": (V, "    Some(((ov / ramp).min(1.0) * f, ov >= ramp))", "    Some(((ov / ramp).min(1.0) * f, true))"),
    "ramp_no_cap": (V, "    Some(((ov / ramp).min(1.0) * f, ov >= ramp))", "    Some(((ov / ramp) * f, ov >= ramp))"),
    "over_counted_any": (V, "    Over { count: worst.iter().filter(|x| x.2).count(), graded: worst.iter().map(|x| x.1).sum() }", "    Over { count: worst.len(), graded: worst.iter().map(|x| x.1).sum() }"),
    "pair_ramp_off": (V, "        if let Some((f, c)) = price(ov, pitch, ramp) {", "        if let Some((f, c)) = price(ov, pitch, 0.0) {"),
    "best_off": (F, "            if self.packer.via_best && o.graded > 0.0 {", "            if false && self.packer.via_best && o.graded > 0.0 {"),
    "best_takes_worse": (F, "                    if oq.graded < o.graded {", "                    if oq.graded > o.graded {"),
    "cands_no_room_filter": ("engine/src/topo/pack.rs", ".filter(|&q| super::cutflow::point_free(self.pr, self, q, net, s) >= 0.5 * d)", ".filter(|&q| super::cutflow::point_free(self.pr, self, q, net, s) >= f64::NEG_INFINITY)"),
    "ramp_in_compass_off": (F, "        self.cost[self.fx.vbase + w] > 0.0 && (self.fx.packer.via_ramp <= 0.0 || self.fx.via_over(&self.a, w).count > 0)", "        self.cost[self.fx.vbase + w] > 0.0"),
    "pour_extra_flat": (P, "            g += 1.0 + EXTRA_PAD * (new - 1) as f64;", "            g += 1.0;"),
    "pour_base_ignored": (P, "        let new = c.pads.iter().filter(|p| !base.cut_pads.contains(p)).count();", "        let new = c.pads.len();"),
    "pour_part_dropped": (O, ".with_neck(parts.neck).with_pour(parts.pour).shorts_ranked", ".with_neck(parts.neck).shorts_ranked"),
    "pour_no_count": (O, "        Predicted { count: self.count + x.0, graded: self.graded + x.1, parts, ..self }", "        Predicted { count: self.count, graded: self.graded + x.1, parts, ..self }"),
    # (2b) via_disc_height
    "dh_pair_off": (P2, "    if ha != 0.0 || hb != 0.0 {", "    if false && (ha != 0.0 || hb != 0.0) {"),
    "dh_lean_a_sign": (P2, "gap - b.lean * ha", "gap + b.lean * ha"),
    "dh_lean_b_sign": (P2, "gap + a.lean * hb", "gap - a.lean * hb"),
    "dh_dd_sum": (P2, "        let dh = ha - hb;", "        let dh = ha + hb;"),
    "dh_track_terms_kept": (P2, "let (ta, tb) = (!(a.w == 0.0 && da > 0.0), !(b.w == 0.0 && db > 0.0));", "let (ta, tb) = (true, true);"),
    "dh_margin_on_line": (P2, "        let off = if h != 0.0 {", "        let off = if false {"),
    "dh_margin_no_early0": (P2, "                if h != 0.0 && poly_dist(at(0.0), &ob.poly) >= req {", "                if false && h != 0.0 && poly_dist(at(0.0), &ob.poly) >= req {"),
    "dh_disc_alone_track_margin": (P2, "    if i.w == 0.0 {\n        ma\n", "    if false {\n        ma\n"),
    "dh_lean_unoriented": (P2, "    let u = if ed.cross(u) < 0.0 { -u } else { u };", "    let u = u;"),
    "dh_heights_off": (F, "            if self.packer.via_disc_h {", "            if false {"),
    "dh_heights_farther": (F, "            if h.abs() < best[j] {", "            if h.abs() > best[j] {"),
    "dh_model_margin_on_line": (F, "        if it.disc[2] != 0.0 {\n            return super::pack::disc_margin", "        if false {\n            return super::pack::disc_margin"),
    "dh_layers_changed_h": (F, " && od[2].to_bits() == nd[2].to_bits();", ";"),
}
TESTS = ["--lib", "--test", "topo_cm1_room", "--test", "topo_disc_height", "--", "viacut", "pourplan", "optimise::tests::the_pour_part", "best_point",
         "via_prices_stay_fresh", "height", "a_disc_against", "a_track_lean", "a_track_with_its_own", "the_end_margin_is"]


def run(out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    only = [x for x in os.environ.get("TW4R2_MUT_ONLY", "").split(",") if x]
    for name, (f, old, new) in M.items():
        if only and name not in only:
            continue
        p = ROOT / f
        src = p.read_text()
        if src.count(old) != 1:
            rows.append(f"{name} stale ({src.count(old)} hits)")
            print(rows[-1], flush=True)
            continue
        p.write_text(src.replace(old, new))
        try:
            r = subprocess.run(["cargo", "test", "--release", *TESTS], cwd=ROOT / "engine", capture_output=True, text=True, timeout=2400,
                               env={**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target-mut")})
            (out / f"{name}.log").write_text(r.stdout[-20000:] + r.stderr[-20000:])
            build_fail = "error[" in r.stderr and "test result" not in r.stdout
            rows.append(f"{name} {'BUILD-FAIL' if build_fail else ('killed' if r.returncode else 'SURVIVED')} rc {r.returncode}")
        finally:
            p.write_text(src)
        print(rows[-1], flush=True)
    (out / "summary.txt").write_text("\n".join(rows) + "\n")
    return 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["list"]:
        print("\n".join(M))
    else:
        sys.exit(run(Path(sys.argv[2])))
