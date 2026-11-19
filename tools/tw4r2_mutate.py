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
}
TESTS = ["--lib", "--test", "topo_cm1_room", "--", "viacut", "pourplan", "optimise::tests::the_pour_part", "best_point", "via_prices_stay_fresh"]


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
