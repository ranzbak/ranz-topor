"""grid-snap mutants: each one breaks one piece of the change; the targeted release tests must fail on it.

    python tools/gridsnap_mut.py <tree copy> [ids...]     (run in a tree COPY with its own CARGO_TARGET_DIR)

Prints one line per mutant: id, killed / SURVIVED, the failing tests.
"""
import re
import subprocess
import sys
from pathlib import Path

M = {
    "G1": ("engine/src/topo/octi.rs", "                            if b > 0.0 {\n                                c += gv;\n                            }\n", ""),
    "G2": ("engine/src/topo/octi.rs", "        if let Some(l) = lat {\n            let base = nodes.len();", "        if let Some(l) = lat.filter(|_| false) {\n            let base = nodes.len();"),
    "G3": ("engine/src/topo/octi.rs", "&& !off_grid {", "&& !(off_grid && false) {"),
    "G4": ("engine/src/topo/octi.rs", "    let Some(l) = lat.filter(|l| l.square()) else { return out };", "    let Some(l) = lat.filter(|_| false) else { return out };"),
    "G5": ("engine/src/topo/viaopt.rs", "        if r < 0.0 || g + price * a < g0 {", "        if r < 0.0 {"),
    "G6": ("engine/src/topo/octi.rs", "    grid.is_finite() && grid < plain - 1e-6\n", "    grid.is_finite() && grid <= plain + 1e-6\n"),
    "G7": ("engine/src/topo/octi.rs", "                                    c += if k == 0 { gnode[i] } else { gprice(from) };", "                                    c += 0.0;"),
    # r2
    "G9": ("engine/src/topo/draw.rs", "    [viol_key(vs).0, hole, vs.iter()", "    [0.0, hole, vs.iter()"),
    "G13": ("engine/src/topo/draw.rs", "    [viol_key(vs).0, hole, vs.iter()", "    [viol_key(vs).0, 0.0 * hole, vs.iter()"),
    "G14": ("engine/src/topo/draw.rs", "v.kind == \"hole_clearance\" && v.depth_nm > margin)", "v.kind == \"hole_clearance\")"),
    "G10": ("engine/src/topo/viaopt.rs", "        if !ok_k(k) || !ok_q(q) {", "        if !ok_k(k) {"),
    "G11": ("engine/src/topo/pourplan.rs", "    if pour_cuts(model, &now, pb).1 <= base + 1e-9 {", "    if true || pour_cuts(model, &now, pb).1 <= base + 1e-9 {"),
    "G12": ("engine/src/topo/pourplan.rs", "    for &(w, i, s0) in moves.iter().filter(|m| bound.contains(&m.0)) {", "    for &(w, i, s0) in moves.iter().filter(|m| bound.contains(&m.0) && false) {"),
    # r3
    "G15": ("engine/src/topo/draw.rs", "    } else if via && (v.drill || v.against", "    } else if via && (v.against"),
    "G16": ("engine/src/check.rs", "required: r, drill: is_via }),", "required: r, drill: false }),"),
    # r6: the grid form never takes more planned room
    "G18": ("engine/src/topo/octi.rs", "    grid <= plain + 1.0\n", "    true\n"),
    # r7 (verifier F9): the room rule's wiring
    "W1": ("engine/src/topo/octi.rs", "    grid_keeps_room(plain_room(plain_form, p0, pg), pg[1])\n", "    true\n"),
    "W2": ("engine/src/topo/octi.rs", "    fallback_takes(p0[0], pg[0]) && prefer_takes(plain_form, p0, pg)\n", "    fallback_takes(p0[0], pg[0])\n"),
    "W3": ("engine/src/topo/octi.rs", "                    price[1] += room_f(from, e);", "                    price[1] += 0.0 * room_f(from, e);"),
    "W4": ("engine/src/topo/octi.rs", "if g.is_some() && pg[1] <= 0.0 {", "if g.is_some() && pg[1] <= 1.0e30 {"),
    "W5": ("engine/src/topo/octi.rs", "    if plain_form {\n        p0[1]\n    } else {\n        pg[2]\n    }\n", "    p0[1]\n"),
    # r4 (verifier V3): force_via's call site back to one sum of amounts
    "G17": ("engine/src/topo/draw.rs", "let key = force_via_key(&vs, side + reach, self.opts.margin_nm.max(0) as f64);",
            "let key = [0.0, 0.0, vs.iter().map(Viol::amount).sum::<f64>() + side + reach];"),
}
TESTS = ["grid", "octilinear", "fallback", "force_via", "deficit", "pour", "viol_kind"]  # grid covers grid_keeps_room


def main() -> int:
    tree = Path(sys.argv[1])
    ids = sys.argv[2:] or list(M)
    for mid in ids:
        f, old, new = M[mid]
        p = tree / f
        src = p.read_text()
        if src.count(old) != 1:
            print(mid, "BAD PATCH (pattern count", src.count(old), ")", flush=True)
            continue
        p.write_text(src.replace(old, new))
        try:
            sel = ["--test", "check_via_holes"] if mid == "G16" else ["--lib", "--", *TESTS]
            r = subprocess.run(["cargo", "test", "--release", *sel], cwd=tree / "engine",
                               capture_output=True, text=True, timeout=1200)
            out = r.stdout + r.stderr
            failed = re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)
            if r.returncode != 0 and not failed:
                print(mid, "killed (build/other)", out[-400:].replace("\n", " | "), flush=True)
            else:
                print(mid, "killed" if failed else "SURVIVED", failed, flush=True)
        finally:
            p.write_text(src)
    return 0


if __name__ == "__main__":
    sys.exit(main())
