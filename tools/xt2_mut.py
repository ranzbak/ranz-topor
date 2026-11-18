#!/usr/bin/env python3
"""exist-term r2-r4 mutants: `xt2_mut.py list` / `xt2_mut.py check <tree>` / `xt2_mut.py <name> apply|revert <tree>`.
One mutant per rule this round changed (fix round 2 of the r1 verifier's findings); each must fail a branch test:
engine/tests/topo_exist_term.rs, the lib tests topo::board::tests / topo::tidy::tests / core unsupported_ends, or (py_*)
tests/test_drcloop.py / tests/plugin/test_run_flow.py. Apply = exact text replace (count must be 1); revert = from the
saved copy (a reverse replace fails for deletions, r1 lesson)."""
import shutil
import sys
from pathlib import Path

C, B, T, R = "engine/src/topo/core.rs", "engine/src/topo/board.rs", "engine/src/topo/tidy.rs", "engine/src/route.rs"
D, O = "engine/src/topo/draw.rs", "engine/src/topo/octi.rs"
M = {
    "dang_retain_off": (C, "                if exist_dang.contains(&c.a) {\n                    src.retain(|e| e.v == c.a);\n                }\n                if exist_dang.contains(&c.b) {\n                    dst.retain(|e| e.v == c.b);\n                }\n", ""),
    "lead_margin_off": (B, "\n    let r = 0.5 * w + margin;\n", "\n    let r = 0.5 * w;\n"),
    "lead_margin_core_off": (C, "lead_margin_nm: opts.margin_nm.max(0) as f64 + super::pack::DRAW_TOL_NM, ..d }", "lead_margin_nm: 0.0, ..d }"),
    "exit_more_off": (B, "exit_stub(&obstacles, &kept_obs, &src,", "exit_stub(&obstacles, &[], &src,"),
    "tidy_4a_off": (T, "        for (ki, k) in model.kept.iter().enumerate() {\n            let Some((ka, kb)) = kseg(k) else { continue };", "        for (ki, k) in model.kept.iter().enumerate().take(0) {\n            let Some((ka, kb)) = kseg(k) else { continue };"),
    "tidy_locked_off": (T, "            if k.locked || ka == kb {", "            if ka == kb {"),
    "tidy_width_off": (T, "                if t.width != k.width || si.0 == si.1 {", "                if si.0 == si.1 {"),
    "tidy_touched_off": (T, "                if !approx_collinear(si, (ka, kb)) || touched(&pairs, &gone, k.net, k.layer, j, i, ki) {", "                if !approx_collinear(si, (ka, kb)) {"),
    "tidy_collinear_off": (T, "                if !approx_collinear(si, (ka, kb)) || touched(", "                if touched("),
    "tidy_via_off": (T, "                if via_pts.get(&k.net).is_some_and(|vs| vs.iter().any(|(p, ls)| *p == j && via_on(ls, k.layer, &cu))) || in_own_pad(k.net, k.layer, j, &mut pad_sk) {", "                if in_own_pad(k.net, k.layer, j, &mut pad_sk) {"),
    "tidy_pad_off": (T, "via_on(ls, k.layer, &cu))) || in_own_pad(k.net, k.layer, j, &mut pad_sk) {", "via_on(ls, k.layer, &cu))) {"),
    "tidy_fold_off": (T, "                    continue; // a fold-back is no continuation\n", ""),
    "tidy_gone_off": (T, "                !gone.contains(&ki) && k.net == net && k.layer == layer && {", "                k.net == net && k.layer == layer && {"),
    "tidy_once_off": (T, "                st.replaced_kept.push(k.uuid.clone());\n                break;\n", "                st.replaced_kept.push(k.uuid.clone());\n"),
    "remove_fwd_off": (R, "        r.remove_uuids.extend(st.replaced_kept.iter().cloned());\n", ""),
    "reason_off": (C, "            (ta, tb) => fail(&mut out, i, unsupported_ends(ta, tb)),", "            (_ta, _tb) => fail(&mut out, i, \"an existing via or track end joined to a pour: not supported by the topological core yet (exist-term covers pads, vias and tracks)\"),"),
    "reason_side_off": (C, "            what(if pour(ta) { tb } else { ta })", "            what(tb)"),
    "three_points_off": ("engine/src/geom/prim.rs", "if cr.abs() < 1e-6 * v.len2() || cr == 0.0 {", "if cr.abs() < 1e-6 * v.len2() {"),
    # r3 (dangling_lead_via): the via room of a cut lead
    "via_core_off": (C, "let via = if opts.dangling_lead_via {", "let via = if false && opts.dangling_lead_via {"),
    "via_layers_off": (B, ".filter(|ob| ob.net != Some(net) && (rv > 0.0 || ob.drilled || ob.layers.has(li)) && near(&ob.poly))", ".filter(|ob| ob.net != Some(net) && (ob.drilled || ob.layers.has(li)) && near(&ob.poly))"),
    "via_hole_off": (B, "(0.5 * via.0 + margin, 0.5 * via.1 + margin + hole_clear)", "(0.5 * via.0 + margin, 0.5 * via.1 + margin)"),
    # r4 (lead decision: the via end only where the plan changes layer at the lead end; exact via-end search;
    # verifier r3 survivors kept_layers_off / via_min_off / via_edge_off / via_grid2 and the I2C_SDA fold-back)
    "kept_layers_off": (B, ".chain(kept.iter().filter(|k| k.net != net && (rv > 0.0 || k.li == li) && near(&k.poly))", ".chain(kept.iter().filter(|k| k.net != net && (k.li == li) && near(&k.poly))"),
    "via_edge_off": (B, "via_end_on(&disc, osegs, rv + edge, at, d, ts)", "via_end_on(&disc, osegs, edge, at, d, ts)"),
    "via_starts_off": (B, "std::iter::once(hi).chain(bad.iter().map(|b| b.0).filter(|&t| (0.0..=hi).contains(&t))).chain(std::iter::once(0.0))", "std::iter::once(hi).chain(std::iter::once(0.0))"),
    "via_slab_off": (B, "            if lo < up {\n                iv.push((lo, up));", "            if false && lo < up {\n                iv.push((lo, up));"),
    "via_cover_off": (B, "        if bad.iter().any(|&(lo, up)| lo + 1e-3 < c && c < up - 1e-3) {", "        if false {"),
    "lead_size_order_rev": ("engine/src/vias.rs", "    let v = via_sizes(rules, net, opts);\n    v.iter().enumerate()", "    let mut v = via_sizes(rules, net, opts);\n    v.reverse();\n    v.iter().enumerate()"),
    "lead_size_drill_off": ("engine/src/vias.rs", ".filter(|&(i, z)| i == 0 || std::f64::consts::PI * z.drill as f64 >= width as f64)", ".filter(|&(_i, _z)| true)"),
    "lead_size_fallback_off": (B, "                for z in via.iter().skip(1) {", "                for z in via.iter().skip(1).take(0) {"),
    "lead_via_draw_off": (D, "if let Some((end, (ve, sz))) = self.lead_via_site(w, i, ka, plan) {", "if let Some((end, (ve, sz))) = self.lead_via_site(w, i, ka, plan).filter(|_| false) {"),
    "lead_via_pass_off": (D, "if (end == 0 && (i != 0 || ka != 0)) || (end == 1 && (i != last || plan.vias[i].1 < n))", "if (end == 0 && i != 0) || (end == 1 && i != last)"),
    "lead_run_skip_off": (D, "            if (r == 0 && at_lead(0, 0)) || (r == plan.layers.len() - 1 && r > 0 && at_lead(1, r - 1)) {\n                continue;", "            if false && ((r == 0 && at_lead(0, 0)) || (r == plan.layers.len() - 1 && r > 0 && at_lead(1, r - 1))) {\n                continue;"),
    "lead_to_via_off": (D, "let q = if shared { q } else { self.lead_via.get(&(w, end)).copied().unwrap_or(q) };", "let q = if shared { q } else { q };"),
    "lead_in_off": (O, "            last[0][8] = (u, f64::INFINITY);\n", ""),
    "kept_end_not_free": ("engine/src/topo/exact.rs", " && !self.is_kept_end(i);", ";"),
    "lead_cut_r3": (B, "                if std::env::var_os(\"TOPO_KEPTDBG\").is_some() {\n                    let e = at + d * l;", "                let l = lv.unwrap_or(l);\n                if std::env::var_os(\"TOPO_KEPTDBG\").is_some() {\n                    let e = at + d * l;"),
    "lead_shared_off": (D, "let shared = self.st.wires.iter().enumerate().any(|(x, o)| x != w && o.alive && (o.pa == ob || o.pb == ob));", "let shared = false;"),
    "lead_out_off": (O, "                                    c += bend(v, len, -ul);\n", ""),
    "py_route_fwd_off": ("plugin/topor/run.py", "\"remove_uuids\": list(res.get(\"remove_uuids\") or [])", "\"remove_uuids\": []"),
    "py_loop_ret_off": ("toporlib/drcloop.py", "                      remove_uuids=remove_uuids)", "                      remove_uuids=[])"),
    "py_commit_off": ("plugin/topor/run.py", "remove_uuids=list(ripped) + [u for u in out.replaced if u not in set(ripped)], items=", "remove_uuids=list(ripped), items="),
}


def main():
    if sys.argv[1] == "list":
        print(" ".join(M))
        return
    if sys.argv[1] == "check":
        bad = 0
        for k, (f, old, _) in M.items():
            n = (Path(sys.argv[2]) / f).read_text().count(old)
            bad += n != 1
            print(k, n)
        sys.exit(1 if bad else 0)
    name, op, tree = sys.argv[1], sys.argv[2], Path(sys.argv[3])
    f, old, new = M[name]
    p = tree / f
    keep = tree / (f + ".mutsave")
    if op == "apply":
        s = p.read_text()
        assert s.count(old) == 1, f"{name}: pattern count {s.count(old)}"
        shutil.copyfile(p, keep)
        p.write_text(s.replace(old, new))
    else:
        shutil.copyfile(keep, p)
        keep.unlink()


if __name__ == "__main__":
    main()
