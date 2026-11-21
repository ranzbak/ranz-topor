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
    "G6": ("engine/src/topo/octi.rs", "    grid < plain\n", "    grid <= plain\n"),
    "G7": ("engine/src/topo/octi.rs", "                                    c += if k == 0 { gnode[i] } else { gprice(from) };", "                                    c += 0.0;"),
}
TESTS = ["grid", "octilinear", "fallback"]


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
            r = subprocess.run(["cargo", "test", "--release", "--lib", "--", *TESTS], cwd=tree / "engine",
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
