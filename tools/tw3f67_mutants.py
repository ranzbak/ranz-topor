#!/usr/bin/env python3
"""tw3-f67 mutants: apply mutant K to the tree at ROOT (in place) or list them.

  tw3f67_mutants.py list            ids, one per line
  tw3f67_mutants.py apply ROOT K    apply mutant K (exit 3 when the anchor is not found exactly once)
Each mutant weakens one F6/F7 prefilter or cache key; the branch tests must fail on every one.
"""
import sys, pathlib

P, X, D, O = "engine/src/topo/planes.rs", "engine/src/topo/exact.rs", "engine/src/topo/draw.rs", "engine/src/topo/octi.rs"
M = {
    # F7 Ring
    "r1_far_shrink": (P, "let q = r + 1.0;", "let q = 0.5 * r;"),
    "r2_pip_band_off": (P, "for &i in &self.bands[self.band(p.y)] {", "for &i in &self.bands[self.band(p.y + self.bh)] {"),
    "r3_insert_band_excl": (P, "for k in r.band(a.y.min(b.y))..=r.band(a.y.max(b.y)) {", "for k in r.band(a.y.min(b.y))..r.band(a.y.max(b.y)) {"),
    "r4_pip_box_shrink": (P, "if p.x < self.bb.min.x || p.x > self.bb.max.x ||", "if p.x < self.bb.min.x + 1000.0 || p.x > self.bb.max.x ||"),
    "r5_hole_far_dropped": (P, "(!h.pip(p) && h.far(p, r))", "!h.pip(p)"),
    "r6_cell_rows_shrink": (P, "for y in self.cell_y(lo.y)..=self.cell_y(hi.y) {", "for y in self.cell_y(p.y)..=self.cell_y(hi.y) {"),
    # F6 Near
    "n1_need_shrink": (X, "let g = nr.c + 0.5 * sw + 1.0;", "let g = 0.5 * nr.c;"),
    "n2_ext_dropped": (X, "nr.slot(bb.min.x - g - nr.ext - 1.0, nr.lo.x, nr.nx)", "nr.slot(bb.min.x - g, nr.lo.x, nr.nx)"),
    "n3_c_min": (X, "let c = ents.iter().map(|&i| self.need_of(i, wp, 0.0)).fold(0.0, f64::max);", "let c = ents.iter().map(|&i| self.need_of(i, wp, 0.0)).fold(f64::INFINITY, f64::min);"),
    # F6 strip triangle grid
    "s1_tri_pad_zero": (X, "const TRI_PAD: f64 = 2.0;", "const TRI_PAD: f64 = 0.0;"),
    "s2_outside_shrink": (X, "if p.x < g.lo.x || p.x > g.hi.x ||", "if p.x < g.lo.x + 3.0 || p.x > g.hi.x ||"),
    "s3_cells_x_excl": (X, "for x in g.slot(b.min.x - TRI_PAD, lo.x, nx)..=g.slot(", "for x in g.slot(b.min.x - TRI_PAD, lo.x, nx)..g.slot("),
    # F6 octilinear shape cache
    "o1_pre_across_nodes": (O, "            for i in 0..n {\n                let mut js: Option<Vec<usize>> = None;\n                let mut pre: Vec<Option<Vec<Shape>>> = Vec::new();", "            let mut pre: Vec<Option<Vec<Shape>>> = Vec::new();\n            for i in 0..n {\n                let mut js: Option<Vec<usize>> = None;"),
    "o2_legal_assumed": (O, "Shape { s, legs, legal: None }", "Shape { s, legs, legal: Some(true) }"),
    "o3_follow_from_start": (O, "=> follow(from, e, kk),", "=> follow(p, e, kk),"),
    # F6 cached boxes, memo keys
    "b1_box_shrink": (D, "self.ent_box.push(core_box);", "self.ent_box.push(core_box.grow(-0.05e6));"),
    "k1_repair_key_a_only": (D, "let key = [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()];\n                if let Some(&worst) = memo.get(&key) {", "let key = [a.x.to_bits(), a.y.to_bits(), a.x.to_bits(), a.y.to_bits()];\n                if let Some(&worst) = memo.get(&key) {"),
    "k2_exact_key_a_only": (X, "let key = [a.x.to_bits(), a.y.to_bits(), b.x.to_bits(), b.y.to_bits()];\n                let ok = *seg_ok", "let key = [a.x.to_bits(), a.y.to_bits(), 0, 0];\n                let ok = *seg_ok"),
}

if sys.argv[1] == "list":
    print("\n".join(M))
    sys.exit(0)
root, k = pathlib.Path(sys.argv[2]), sys.argv[3]
f, old, new = M[k]
p = root / f
s = p.read_text()
if s.count(old) != 1:
    print(f"{k}: anchor found {s.count(old)} times in {f}", file=sys.stderr)
    sys.exit(3)
p.write_text(s.replace(old, new))
