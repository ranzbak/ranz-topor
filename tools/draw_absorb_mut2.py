#!/usr/bin/env python3
"""draw-absorb A1b/A3 mutants (VM unit): each applied to a copy of the repo tree under <out>/tree-<k>, then
cargo test --release --test topo_draw_absorb; killed = a test fails.  usage: draw_absorb_mut2.py <out> <k> name...
(no names = all)."""
import json, os, shutil, subprocess, sys
OUT, K = sys.argv[1], sys.argv[2]
ONLY = sys.argv[3:]
WT = os.getcwd()
TREE, TGT = f"{OUT}/tree-{K}", f"{OUT}/target-{K}"
E, D, P = "engine/src/topo/erbs.rs", "engine/src/topo/draw.rs", "engine/src/topo/padend.rs"
MUTS = [
 # A1b
 ("a1b_tip_off", P, "                    (true, Some((wn, _))) => self.plan_neck(w).map(|n| super::exact::Neck { min_w: wn, ends: n.ends }),", "                    (true, Some((wn, _))) => { let _ = wn; None }"),
 ("a1b_no_meter", P, "                            self.joint.unnecked.push((w, r, dropped, room, \"tip\"));", "                            let _ = room;"),
 ("a1b_no_grow", P, "self.fit_neck_with(w, lid, pts, z, z.min_w, true)", "self.fit_neck_with(w, lid, pts, z, z.min_w, false)"),
 ("a1b_no_fit", P, "let nk = if self.opts.neck_trim { self.fit_neck_with(w, lid, pts, z, z.min_w, true) } else { *z };", "let nk = *z;"),
 ("a1b_octi_off", D, "let a1b = adopted.as_ref().is_some_and(|x| x.zones.is_none() && self.opts.reserved_keep_neck", "let a1b = adopted.as_ref().is_some_and(|x| x.zones.is_none() && false"),
 ("a1b_octi_always_kept", D, "                                if main.as_ref().is_some_and(|(_, pn)| pn.is_some_and(|p| p.0 <= z.min_w + 0.5)) {", "                                if false {"),
 ("a1b_room_zero", D, "                if inside && self.via_plan_price(w, 0, last, q, wi.w) > self.via_plan_price(w, 0, last, q, wn) {", "                if false && inside {"),
 # A3
 ("a3_off", E, "        if eo.band_passes > 0 {\n            let pos", "        if false {\n            let pos"),
 ("a3_accept_all", E, ".filter(|&y| price(&es, &at, &nvs, y) >= price(&old_es, &at, &old_vs, y) - 1.0).collect();", ".filter(|&y| y == usize::MAX).collect();"),
 ("a3_no_len_guard", E, "                    e.len >= old.len - 1.0 && e.through.iter().all(|f| old.through.contains(f))", "                    e.through.iter().all(|f| old.through.contains(f))"),
 ("a3_no_through_guard", E, "                    e.len >= old.len - 1.0 && e.through.iter().all(|f| old.through.contains(f))", "                    e.len >= old.len - 1.0"),
 ("a3_side_flip", E, "            gates.push(Gate { a: x, b: x, w: s.w, src: GateSrc::Band(wl.stretch), p: root, q: root, p_left: left });\n            let dips = spoke_crossed(band, x, root);\n            cand.push((x, left,", "            gates.push(Gate { a: x, b: x, w: s.w, src: GateSrc::Band(wl.stretch), p: root, q: root, p_left: !left });\n            let dips = spoke_crossed(band, x, root);\n            cand.push((x, !left,"),
 ("a3_earlier_yields", E, "let (y, p) = if (rank(a), a) > (rank(b), b) { (a, b) } else { (b, a) };", "let (y, p) = if (rank(a), a) < (rank(b), b) { (a, b) } else { (b, a) };"),
 ("a3_no_swap", E, "                let (y, p) = if failed.contains(&(y, p)) { (p, y) } else { (y, p) };", "                let (y, p) = (y, p);"),
 ("a3_no_viol_price", E, "            len + PASS_VIOL_W * v\n", "            len + 0.0 * v\n"),
]
res = {}
if os.path.isdir(TREE):
    shutil.rmtree(TREE)
shutil.copytree(WT, TREE, ignore=shutil.ignore_patterns(".git", "target", "scratch"), symlinks=True)
files = {m[1] for m in MUTS}
for name, f, old, new in MUTS:
    if ONLY and name not in ONLY:
        continue
    for g in files:
        shutil.copy(os.path.join(WT, g), os.path.join(TREE, g)); os.utime(os.path.join(TREE, g))
    s = open(os.path.join(WT, f)).read()
    if s.count(old) != 1:
        res[name] = f"NOT APPLIED ({s.count(old)})"; print(name, res[name], flush=True); continue
    dst = os.path.join(TREE, f)
    open(dst, "w").write(s.replace(old, new)); os.utime(dst)
    env = dict(os.environ, CARGO_TARGET_DIR=TGT)
    p = subprocess.run(["cargo", "test", "--release", "--manifest-path", TREE + "/engine/Cargo.toml", "--test", "topo_draw_absorb"], env=env, capture_output=True, text=True)
    out = p.stdout + p.stderr
    res[name] = "COMPILE ERROR" if ("error[E" in out or "could not compile" in out) else ("killed" if p.returncode != 0 else "SURVIVED")
    fails = [l for l in out.splitlines() if l.startswith("test ") and "FAILED" in l]
    print(name, res[name], fails, flush=True)
json.dump(res, open(f"{OUT}/mut-{K}.json", "w"), indent=1)
