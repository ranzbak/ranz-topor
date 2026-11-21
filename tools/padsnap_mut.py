"""pad-snap r1 mutants: apply one mutation to a tree copy, run the targeted test, print killed / survived.

    python tools/padsnap_mut.py <tree> <id> [<id> ...]      (the tree is modified in place: use a copy)

Each mutant: a list of (file, old text, new text); killed = topo_pad_snap or the lib unit tests (stub_price,
off_centre_pad_ends) fail. M0 = M12 + M2 (no leave shapes, no stub price: main's DP).
The tree's files are restored from a backup after each mutant (no git needed).
"""
from __future__ import annotations

import subprocess
import sys
from pathlib import Path

OCTI = "engine/src/topo/octi.rs"
MUT = {
    "M2": (OCTI, "    STUB_NM * (1.0 + (j - c).len() / ext.max(1.0))\n", "    0.0 * STUB_NM * (1.0 + (j - c).len() / ext.max(1.0))\n"),
    "M3": (OCTI, "pub const LEAVE_NM: f64 = 20_000.0;", "pub const LEAVE_NM: f64 = -20_000.0;"),
    "M4": ("engine/src/topo/tidy.rs", "            if d > 1_000.0 {\n                n += 1;", "            if d > 1e12 {\n                n += 1;"),
    "M5": (OCTI, "c += axis_price(&exits[0], v, len) + stub(0, sh.s[0].0);", "c += axis_price(&exits[0], v, len);"),
    "M6": (OCTI, "c += axis_price(&exits[1], v, len) + stub(1, if k == 0 { p } else { sh.s[k - 1].0 });",
           "c += axis_price(&exits[1], v, len);"),
    # r2b: modes 1 (axis leg to the leave point, M1/M7) and 2 (45 deg side exit, M8) are now two of the 8 directions
    # of mode 3: M12 = no mode 3 shapes, M13 = the DP's early return without the in-pad stub guard, M14 = the leave
    # point's round end inside the pad instead of out of it.
    "M12": (OCTI, "            if mode == 3 {\n", "            if mode == 3 {\n                return;\n"),
    "M13": (OCTI, " && !self.run_pad_stubs(w, k0, k1, &taut).iter().any(|&x| x)", ""),
    "M14": (OCTI, "    let lim = 0.5 * w - LEAVE_NM;", "    let lim = 0.5 * w + LEAVE_NM;"),
    # r2b-2: M9 (forced in-pad stub exemption), M10 (forced form judged without the stub legs) and M11 (the
    # price of a later leg inside the own pad) were live branches no fixture reached: removed from the code.
}
MUTS = {k: [v] for k, v in MUT.items()}
MUTS["M0"] = [MUT["M12"], MUT["M2"]]
MUTS["none"] = []  # the unmutated tree: every test must pass


def run(tree: Path, mid: str) -> str:
    for f, old, new in MUTS[mid]:
        p = tree / f
        s = p.read_text()
        if s.count(old) == 0:
            return f"{mid} ERROR (pattern not found in {f})"
        p.write_text(s.replace(old, new))
    rc, tail = 0, []
    for args in (["--test", "topo_pad_snap"], ["--lib", "--", "stub_price", "leave_point", "off_centre_pad_ends", "forced_", "pad_stub_run"]):
        r = subprocess.run(["cargo", "test", "--release", *args], cwd=tree / "engine", capture_output=True, text=True)
        tail += [ln for ln in (r.stdout + r.stderr).splitlines() if ln.startswith("test ") and ("FAILED" in ln or " ok" in ln)]
        if "could not compile" in r.stderr:
            return f"{mid} killed (build error)"
        rc |= r.returncode
    return f"{mid} {'killed' if rc != 0 else 'SURVIVED'}: " + "; ".join(tail)


def main() -> int:
    tree = Path(sys.argv[1])
    files = {f for m in MUTS.values() for f, _, _ in m}
    keep = {f: (tree / f).read_text() for f in files}
    for mid in sys.argv[2:]:
        try:
            print(run(tree, mid), flush=True)
        finally:
            for f, t in keep.items():
                (tree / f).write_text(t)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
