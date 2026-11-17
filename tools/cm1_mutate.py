#!/usr/bin/env python3
"""Mutants of the cm1-room lines, each killed (or not) by the targeted engine tests.

    tools/cm1_mutate.py list
    tools/cm1_mutate.py run <out_dir>     (VM: each mutant applied, tests run, restored; one line per mutant)
    CM1_MUT_ONLY=a,b ...                  (only these mutants)

A mutant = (file, old text, new text); `old` must occur exactly once (else 'stale')."""
import os, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
V = "engine/src/topo/viacut.rs"
F = "engine/src/topo/refine.rs"
M = {
    "no_via_item": (V, "        its.push((c.via_t, via));\n", "\n"),
    "no_wall": (V, "            (Bound::Pad { net: Some(n), .. }, Some(x)) if n == via.net && x.0 == c.via_t && x.1 == via => Bound::Edge { clear: 0.0 },", "            (Bound::Pad { net: Some(n), .. }, Some(x)) if false && n == via.net && x.0 == c.via_t && x.1 == via => Bound::Edge { clear: 0.0 },"),
    "own_net_crossers": (V, "        if !wi.alive || wi.net == net {\n            continue;\n        }\n        let taut", "        if !wi.alive {\n            continue;\n        }\n        let taut"),
    "chord_closed": (V, "(t > 0.0 && t < 1.0 && (0.0..1.0).contains(&s))", "(t > 0.0 && t < 1.0 && (0.0..=1.0).contains(&s))"),
    "pair_one_half": (V, "let len = norm.len(q - p) - 0.5 * a.w - 0.5 * b.w;", "let len = norm.len(q - p) - 0.5 * a.w;"),
    "per_layer_sum": (V, "            Some(x) => x.1 = x.1.max(f),\n            None => worst.push((c.layer, f)),", "            Some(x) => x.1 += f,\n            None => worst.push((c.layer, f)),"),
    "option_off": ("engine/src/topo/pack.rs", "p.via_cuts = opts.via_cuts.then(|| super::sheet::Norm::for_style(&opts.draw_style));", "p.via_cuts = None;"),
    "no_reprice": (F, "        let vd = self.reprice_vias(changed);\n", "        let vd = 0.0;\n"),
    "layer_any": (F, "            hit || (!any && self.bottom == l as i32)", "            true"),
    "count_off": (F, "        over + self.nosite.iter().sum::<usize>() + self.via_cut_count()", "        over + self.nosite.iter().sum::<usize>()"),
    "price_off": (F, "        self.price * self.via_cut_over(w).graded\n", "        0.0 * self.via_cut_over(w).graded\n"),
    "exit_wide": ("engine/src/route.rs", ".filter(|x| x.1 <= 1.5 * pitch)", ".filter(|x| x.1 <= 2.5 * pitch)"),
    "exit_narrow": ("engine/src/route.rs", ".filter(|x| x.1 <= 1.5 * pitch)", ".filter(|x| x.1 <= 1.0 * pitch)"),
    "class_any_pin": ("engine/src/route.rs", "exits.iter().any(|e| e.split_once(':') == Some((r, n)))", "exits.iter().any(|e| e.split_once(':').map(|x| x.0) == Some(r))"),
    "class_cause_off": ("engine/src/route.rs", "|| cause.starts_with(\"drawing:\") && exit_pad(cause)", "|| false && exit_pad(cause)"),
}
TESTS = ["--lib", "--test", "topo_cm1_room", "--", "viacut", "route::tests", "a_via_in_a_pad_gap", "without_via_cuts"]


def run(out: Path) -> int:
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    only = [x for x in os.environ.get("CM1_MUT_ONLY", "").split(",") if x]
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
