#!/usr/bin/env python3
"""draw-absorb r3: the r2 mutants (rerun on the merge with main acdee25b) plus the r3 lines (A1 zones in
the mid slots, the strict-gain gate of shrink_forced), each on a copy of the tree.
usage: draw_absorb_r3_mut.py <out> [name...] [--check]
each mutant: cargo test --release --test topo_draw_absorb + --lib topo::joint; killed = a test fails
(or the mutant does not build). Writes <out>/mut-r3.json."""
import json, os, shutil, subprocess, sys
OUT = sys.argv[1]
ONLY = [a for a in sys.argv[2:] if not a.startswith("--")]
WT = os.getcwd()
TREE, TGT = f"{OUT}/tree-r3", f"{OUT}/target-r3"
J, D, C, E, X = "engine/src/topo/joint.rs", "engine/src/topo/draw.rs", "engine/src/topo/core.rs", "engine/src/topo/erbs.rs", "engine/src/topo/exact.rs"
MUTS = [
 ("mono_off", J, '        if !super::octi::fd45_off("dab_mono") {', "        if false {"),
 ("stub_off", J, '        if !super::octi::fd45_off("dab_stub") {', "        if false {"),
 ("merge_off", J, "        let mut items = merge_collinear(items);", "        let mut items = items;"),
 ("mono_always_fwd", J, "    Some(if cost(&bwd) < cost(&fwd) { bwd } else { fwd })", "    Some(fwd)"),
 ("mono_always_bwd", J, "    Some(if cost(&bwd) < cost(&fwd) { bwd } else { fwd })", "    Some(bwd)"),
 ("mono_no_early_none", J, "    if same(&fwd) || same(&bwd) {\n        return None;", "    if false {\n        return None;"),
 ("ofb_off", D, '                                if super::octi::fd45_off("dab_ofb") {', "                                if true {"),
 ("supply_all_nets", C, "            if !dr.joint.a1_wires.contains(&w) || wi.w <= dflt + 0.5 {", "            if !dr.joint.a1_wires.contains(&w) {"),
 ("supply_not_a1_only", C, "            if !dr.joint.a1_wires.contains(&w) || wi.w <= dflt + 0.5 {", "            if wi.w <= dflt + 0.5 {"),
 ("supply_flat_sev", C, "    (left, (1.0 - left) * len / MM)", "    (left, len / MM)"),
 ("a3_no_swap", E, "                let (y, p) = if failed.contains(&(y, p)) { (p, y) } else { (y, p) };", "                let (y, p) = (y, p);"),
 # r3: A1 zones in the end slots 0/1 again (the r2 code on the merged neck_items)
 ("slot01_shrink", D, "        for (z, v) in ends.iter_mut().skip(super::exact::MID_ZONE).zip(zs) {", "        for (z, v) in ends.iter_mut().zip(zs) {"),
 ("slot01_joint", J, "                    let mids = super::exact::MID_ZONE..super::exact::NECK_ZONES;", "                    let mids = 0..super::exact::NECK_ZONES;"),
 ("mid_zone_3", X, "pub(super) const MID_ZONE: usize = 2;", "pub(super) const MID_ZONE: usize = 3;"),
 ("shrink_not_strict", D, "            key_le(k, b) && (k.0 < b.0 - 1e-9 || k.1 < b.1 - 1e-9)\n        };", "            key_le(k, b)\n        };"),
]
if "--check" in sys.argv:
    for name, f, old, _ in MUTS:
        print(name, open(os.path.join(WT, f)).read().count(old))
    sys.exit(0)
if os.path.isdir(TREE):
    shutil.rmtree(TREE)
shutil.copytree(WT, TREE, ignore=shutil.ignore_patterns(".git", "target", "scratch"), symlinks=True)
orig = {f: open(os.path.join(TREE, f)).read() for f in {m[1] for m in MUTS}}
res = {}
for name, f, old, new in MUTS:
    if ONLY and name not in ONLY:
        continue
    src = orig[f]
    assert src.count(old) == 1, (name, src.count(old))
    open(os.path.join(TREE, f), "w").write(src.replace(old, new))
    env = dict(os.environ, CARGO_TARGET_DIR=TGT)
    cmd = "cd engine && cargo test --release --test topo_draw_absorb --no-fail-fast 2>&1 && cargo test --release --lib topo::joint 2>&1"
    p = subprocess.run(["bash", "-c", cmd], cwd=TREE, env=env, capture_output=True, text=True)
    out = p.stdout + p.stderr
    failed = [l.split()[1] for l in out.splitlines() if l.startswith("test ") and l.endswith("FAILED")]
    built = "error[" not in out and "could not compile" not in out
    res[name] = {"killed": p.returncode != 0, "built": built, "failed": failed}
    print(name, "killed" if p.returncode != 0 else "SURVIVED", failed if built else "build error", flush=True)
    open(os.path.join(TREE, f), "w").write(src)
json.dump(res, open(f"{OUT}/mut-r3.json", "w"), indent=1)
