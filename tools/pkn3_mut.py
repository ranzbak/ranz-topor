#!/usr/bin/env python3
"""vfy pick-necks r2: mutants of the lines r2 changed. `list` | `apply <id> <engine dir>` | `restore <engine dir>` (exact, unique replace)."""
import sys, pathlib, shutil
A, C = "src/topo/archive.rs", "src/topo/core.rs"
M = {
 "m1_dominates": (A, " || self.shrinkages < o.shrinkages || self.necking < o.necking - 1e-9)", " || self.shrinkages < o.shrinkages)"),
 "m2_dec_tie": (A, "if rest.abs() <= 1e-9 || (rest < 0.0) != (dv < 0.0) {", "if (rest < 0.0) != (dv < 0.0) {"),
 "m3_dec_sign": (A, "if rest.abs() <= 1e-9 || (rest < 0.0) != (dv < 0.0) {", "if rest.abs() <= 1e-9 {"),
 "m4_via_plan_model": (C, "crate::vias::via_sizes(&model_real.rules, n, opts)", "crate::vias::via_sizes(&model.rules, n, opts)"),
 "m5_fullw_no_pair": (C, "let full_w = |ci: usize, net: i32| model_real.rules.diff_pairs.iter().find(|p| (p.p == net || p.n == net) && p.width > 0).map_or(dec.connections[ci].width, |p| p.width);", "let full_w = |ci: usize, _net: i32| dec.connections[ci].width;"),
 "m6_crit_necking_zero": (C, "necking: necking_mm_eq(neck_mm2, neck_vias, opts),", "necking: 0.0,"),
 "m8_round_delta": (C, "        \"necking\" => format!(\"necking {d:+.1} mm-eq\"),\n", ""),
 "m9_bests_json": (C, "\"necking\": (c.necking * 1e3).round() / 1e3, ", ""),
 "m10_base": (A, "Objective::Best { mm_per_via, .. } => c.length_mm + mm_per_via * c.vias as f64 + c.necking,", "Objective::Best { mm_per_via, .. } => c.length_mm + mm_per_via * c.vias as f64,"),
}
if sys.argv[1] == "list":
    print(" ".join(M))
    sys.exit(0)
eng = pathlib.Path(sys.argv[-1])
if sys.argv[1] == "restore":
    for f in (A, C):
        b = eng / (f + ".orig")
        if b.exists():
            shutil.copy(b, eng / f)
            b.unlink()
            (eng / f).touch()
    sys.exit(0)
f, old, new = M[sys.argv[2]]
p = eng / f
t = p.read_text()
assert t.count(old) == 1, (sys.argv[2], t.count(old))
shutil.copy(p, eng / (f + ".orig"))
p.write_text(t.replace(old, new))
p.touch()
print("applied", sys.argv[2])
